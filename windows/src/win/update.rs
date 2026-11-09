//! Обновление из GitHub Releases, как `Updater.swift`. Свой поток: сеть не трогает кадровый цикл.
//! Подписи кода нет, поэтому файл сверяется с отдельной подписью ed25519.
use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use sugrob_core::release::{self, Release, EXE_ASSET, SIG_ASSET};
use windows::core::{w, Error, PCWSTR};
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

use super::{Log, QUIT};

const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// Вшивается при сборке. Пока в файле заглушка, обновления не ставятся.
const PUBLIC_KEY: &[u8] = include_bytes!("../../update_public_key.bin");
const FIRST_CHECK: Duration = Duration::from_secs(30);
const INTERVAL: Duration = Duration::from_secs(6 * 3600);
/// Сбой сети не должен откладывать обновление на 6 часов.
const RETRY: Duration = Duration::from_secs(15 * 60);
const API_LIMIT: usize = 8 << 20;
const EXE_LIMIT: usize = 64 << 20;
const SIG_LIMIT: usize = 1024;
const AFTER_UPDATE: &str = "--after-update";
const WAIT_PREVIOUS_MS: u32 = 10_000;
const WAIT_INSTALL: Duration = Duration::from_secs(15);
/// Версия, которую ставили последней: упавшая при запуске не ставится по кругу.
const LAST_UPDATE_FILE: &str = "last_update";

static INSTALLING: AtomicBool = AtomicBool::new(false);
static CONFIRMED: AtomicBool = AtomicBool::new(false);

static CHECK_NOW: OnceLock<Sender<()>> = OnceLock::new();

/// Пункт меню. У неустановленной копии потока нет, и запрос просто не уходит.
pub fn check_now() {
    if let Some(sender) = CHECK_NOW.get() {
        let _ = sender.send(());
    }
}

fn installed_exe() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let base = std::env::var("LOCALAPPDATA").ok()?;
    release::is_installed_copy(&exe.to_string_lossy(), &base).then_some(exe)
}

pub fn start(mut log: Log) {
    let Some(exe) = installed_exe() else {
        log.write("update: копия не в Programs\\Sugrob, обновления выключены");
        return;
    };
    let mut spawn_log = log.try_clone();
    let (sender, receiver) = mpsc::channel();
    if CHECK_NOW.set(sender).is_err() {
        return;
    }
    let spawned = std::thread::Builder::new().name("update".into()).spawn(move || {
        let mut wait = FIRST_CHECK;
        loop {
            if matches!(receiver.recv_timeout(wait), Err(RecvTimeoutError::Disconnected)) {
                return;
            }
            // Несколько нажатий подряд считаются одной проверкой.
            while receiver.try_recv().is_ok() {}
            wait = match check(&exe, &mut log) {
                Ok(()) => INTERVAL,
                Err(error) => {
                    log.write(&format!("update: {error}"));
                    RETRY
                }
            };
            if QUIT.load(Ordering::Relaxed) {
                return;
            }
        }
    });
    if let (Err(error), Some(log)) = (spawned, spawn_log.as_mut()) {
        log.write(&format!("update: поток не запустился: {error}"));
    }
}

fn check(exe: &Path, log: &mut Log) -> Result<(), String> {
    let list = http_get(w!("api.github.com"), &release::releases_api_path(), API_LIMIT)?;
    let Some(found) = release::newest_windows_release(&list)? else { return Ok(()) };
    if !release::is_newer(&found.version, CURRENT) {
        return Ok(());
    }
    if read_last_update().as_deref() == Some(found.version.as_str()) {
        log.write(&format!("update: {} уже ставилась и не прижилась, пропускаю", found.version));
        return Ok(());
    }
    if !found.has_asset(EXE_ASSET) || !found.has_asset(SIG_ASSET) {
        log.write(&format!("update: в {} нет файла или подписи, пропускаю", found.tag));
        return Ok(());
    }
    if !release::key_is_real(PUBLIC_KEY) {
        log.write(&format!("update: есть {}, но вместо ключа заглушка, установка отменена", found.version));
        return Ok(());
    }

    log.write(&format!("update: загрузка {}", found.version));
    let data = download(&found, EXE_ASSET, EXE_LIMIT)?;
    let signature = download(&found, SIG_ASSET, SIG_LIMIT)?;
    if !release::signature_valid(PUBLIC_KEY, &found.version, &data, &signature) {
        log.write("update: подпись не сошлась, пропускаю");
        return Ok(());
    }
    // Порядок важен: выход главного потока смотрит эти два флага в обратном порядке.
    INSTALLING.store(true, Ordering::SeqCst);
    if QUIT.load(Ordering::SeqCst) {
        INSTALLING.store(false, Ordering::SeqCst);
        return Ok(());
    }
    let outcome = install(exe, &data);
    if outcome.is_ok() {
        if let Err(error) = write_last_update(&found.version) {
            log.write(&format!("update: версия не записана: {error}"));
        }
    }
    INSTALLING.store(false, Ordering::SeqCst);
    if outcome? {
        log.write(&format!("update: установлена {}, перезапуск", found.version));
        QUIT.store(true, Ordering::Relaxed);
    } else {
        log.write(&format!("update: установлена {}, запустится при следующем старте", found.version));
    }
    Ok(())
}

fn last_update_path() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Sugrob").join(LAST_UPDATE_FILE))
}

fn read_last_update() -> Option<String> {
    std::fs::read_to_string(last_update_path()?).ok().map(|text| text.trim().to_string())
}

fn write_last_update(version: &str) -> Result<(), String> {
    let path = last_update_path().ok_or("нет LOCALAPPDATA")?;
    std::fs::write(path, version).map_err(|error| error.to_string())
}

fn download(found: &Release, asset: &str, limit: usize) -> Result<Vec<u8>, String> {
    http_get(w!("github.com"), &found.download_path(asset), limit)
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Запущенный exe нельзя перезаписать, но можно переименовать: он уходит в сторону, новый встаёт на его место.
/// Возвращает false, если файл заменён, но пользователь уже вышел: тогда запуска нет.
fn install(exe: &Path, data: &[u8]) -> Result<bool, String> {
    let (new, old) = (sibling(exe, ".new"), sibling(exe, ".old"));
    std::fs::write(&new, data).map_err(|error| format!("запись нового файла: {error}"))?;
    let _ = std::fs::remove_file(&old);
    if let Err(error) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(format!("текущий файл не переименован: {error}"));
    }
    let placed = std::fs::rename(&new, exe).map_err(|error| format!("новый файл не встал: {error}"));
    let started = placed.and_then(|()| {
        if QUIT.load(Ordering::SeqCst) {
            return Ok(false);
        }
        launch(exe).map(|()| true).map_err(|error| format!("новая версия не запустилась: {error}"))
    });
    if started.is_err() {
        let _ = std::fs::remove_file(&new);
        let _ = std::fs::rename(&old, exe);
    }
    started
}

fn launch(exe: &Path) -> std::io::Result<()> {
    Command::new(exe).arg(AFTER_UPDATE).arg(std::process::id().to_string()).spawn().map(|_| ())
}

/// Главный поток уходит только после замены файла, иначе exe мог бы пропасть совсем.
pub fn finish_install() {
    QUIT.store(true, Ordering::SeqCst);
    let started = Instant::now();
    while INSTALLING.load(Ordering::SeqCst) && started.elapsed() < WAIT_INSTALL {
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn started_after_update() -> bool {
    std::env::args().any(|arg| arg == AFTER_UPDATE)
}

/// Новая версия ждёт выхода старой: иначе ей не достанется горячая клавиша, а файл .old останется занят.
pub fn wait_for_previous() {
    let args: Vec<String> = std::env::args().collect();
    let Some(pid) = args.windows(2).find(|pair| pair[0] == AFTER_UPDATE).and_then(|pair| pair[1].parse::<u32>().ok()) else { return };
    unsafe {
        if let Ok(process) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(process, WAIT_PREVIOUS_MS);
            let _ = CloseHandle(process);
        }
    }
}

/// Остатки прошлого обновления. Сразу после обновления .old ещё нужен для отката.
pub fn clean_up(log: &mut Log) {
    let Ok(exe) = std::env::current_exe() else { return };
    if !started_after_update() {
        remove_old(&exe, log);
    }
    let _ = std::fs::remove_file(sibling(&exe, ".new"));
    let _ = std::fs::remove_file(sibling(&exe, ".bad"));
}

fn remove_old(exe: &Path, log: &mut Log) {
    let old = sibling(exe, ".old");
    if old.exists() {
        match std::fs::remove_file(&old) {
            Ok(()) => log.write("update: старая версия удалена"),
            Err(error) => log.write(&format!("update: старая версия не удалена: {error}")),
        }
    }
}

/// Новая версия отработала кадры: откат больше не нужен. Файл удаляет отдельный поток, не кадр.
pub fn confirm(log: &Log) {
    CONFIRMED.store(true, Ordering::Relaxed);
    if !started_after_update() {
        return;
    }
    let (Ok(exe), Some(mut log)) = (std::env::current_exe(), log.try_clone()) else { return };
    let _ = std::thread::Builder::new().name("update-confirm".into()).spawn(move || remove_old(&exe, &mut log));
}

/// Версия после обновления не запустилась: возвращаем старую. True, если она запущена.
pub fn roll_back(log: &mut Log) -> bool {
    if !started_after_update() || CONFIRMED.load(Ordering::Relaxed) {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else { return false };
    let (old, bad) = (sibling(&exe, ".old"), sibling(&exe, ".bad"));
    if !old.exists() {
        return false;
    }
    let _ = std::fs::remove_file(&bad);
    if let Err(error) = std::fs::rename(&exe, &bad) {
        log.write(&format!("update: откат не удался: {error}"));
        return false;
    }
    if let Err(error) = std::fs::rename(&old, &exe) {
        log.write(&format!("update: откат не удался: {error}"));
        let _ = std::fs::rename(&bad, &exe);
        return false;
    }
    match launch(&exe) {
        Ok(()) => {
            log.write("update: откат на прошлую версию");
            true
        }
        Err(error) => {
            log.write(&format!("update: прошлая версия не запустилась: {error}"));
            false
        }
    }
}

/// Закрывается при выходе из области видимости.
struct Handle(*mut c_void);

impl Handle {
    fn new(raw: *mut c_void, what: &str) -> Result<Self, String> {
        if raw.is_null() {
            Err(format!("{what}: {}", Error::from_win32()))
        } else {
            Ok(Handle(raw))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = WinHttpCloseHandle(self.0);
        }
    }
}

fn http_get(host: PCWSTR, path: &str, limit: usize) -> Result<Vec<u8>, String> {
    let path: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    unsafe {
        // Системный прокси учитывается. Только HTTPS; переход на HTTP WinHTTP запрещает по умолчанию.
        let session = Handle::new(WinHttpOpen(w!("Sugrob-updater"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0), "сессия")?;
        WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 30_000).map_err(|error| format!("тайм-ауты: {error}"))?;
        let connection = Handle::new(WinHttpConnect(session.0, host, INTERNET_DEFAULT_HTTPS_PORT, 0), "соединение")?;
        let request = Handle::new(WinHttpOpenRequest(connection.0, w!("GET"), PCWSTR(path.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE), "запрос")?;
        WinHttpSendRequest(request.0, None, None, 0, 0, 0).map_err(|error| format!("отправка: {error}"))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|error| format!("ответ: {error}"))?;

        let mut status = 0u32;
        let mut size = std::mem::size_of::<u32>() as u32;
        WinHttpQueryHeaders(request.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut status as *mut u32 as *mut c_void), &mut size, std::ptr::null_mut()).map_err(|error| format!("код ответа: {error}"))?;
        if status != 200 {
            return Err(format!("сервер ответил {status}"));
        }

        let mut body = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut c_void, chunk.len() as u32, &mut read).map_err(|error| format!("чтение: {error}"))?;
            if read == 0 {
                return Ok(body);
            }
            if body.len() + read as usize > limit {
                return Err("ответ слишком большой".into());
            }
            body.extend_from_slice(&chunk[..read as usize]);
        }
    }
}
