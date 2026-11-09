//! Подпись обновлений. Секрет: 32 байта в hex в переменной SUGROB_UPDATE_KEY.
//!   update_sign public <файл ключа>        записывает публичный ключ
//!   update_sign sign <версия> <файл> <подпись> <файл ключа>   подписывает и проверяет
use ed25519_compact::{KeyPair, Seed};
use sugrob_core::release::{key_is_real, signature_valid, signed_message};

fn key_pair() -> Result<KeyPair, String> {
    let text = std::env::var("SUGROB_UPDATE_KEY").map_err(|_| "нет переменной SUGROB_UPDATE_KEY")?;
    let text = text.trim();
    if text.len() != 64 || !text.is_ascii() {
        return Err("SUGROB_UPDATE_KEY должен быть 64 символа hex".into());
    }
    let mut seed = [0u8; 32];
    for (index, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).map_err(|_| "SUGROB_UPDATE_KEY не hex")?;
    }
    Ok(KeyPair::from_seed(Seed::new(seed)))
}

fn run(args: &[String]) -> Result<(), String> {
    let pair = key_pair()?;
    let public_key = pair.pk.as_ref();
    match args {
        [command, key_path] if command == "public" => std::fs::write(key_path, public_key).map_err(|error| format!("{key_path}: {error}")),
        [command, version, file, signature_path, key_path] if command == "sign" => {
            let stored = std::fs::read(key_path).map_err(|error| format!("{key_path}: {error}"))?;
            if !key_is_real(&stored) {
                return Err(format!("{key_path}: заглушка вместо ключа"));
            }
            if stored != public_key {
                return Err(format!("{key_path} не соответствует SUGROB_UPDATE_KEY"));
            }
            let data = std::fs::read(file).map_err(|error| format!("{file}: {error}"))?;
            let signature = pair.sk.sign(signed_message(version, &data), None);
            if !signature_valid(&stored, version, &data, signature.as_ref()) {
                return Err("подпись не прошла проверку".into());
            }
            std::fs::write(signature_path, signature.as_ref()).map_err(|error| format!("{signature_path}: {error}"))
        }
        _ => Err("использование: update_sign public <ключ> | sign <версия> <файл> <подпись> <ключ>".into()),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Err(error) = run(&args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
