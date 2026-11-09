//! Правила паузы и экономии, перенос решений из `Governor.swift`.
//! Сбор данных из системы лежит в `win/governor.rs`.

/// Что сейчас известно о системе и настройках.
#[derive(Clone, Copy, Debug, Default)]
pub struct PauseInput {
    pub enabled: bool,
    pub display_off: bool,
    pub locked: bool,
    pub pause_in_editors: bool,
    pub editor_in_front: bool,
    pub fullscreen_in_front: bool,
}

/// Причина паузы для лога. `None`: снег идёт.
pub fn pause_reason(input: &PauseInput) -> Option<&'static str> {
    if !input.enabled {
        Some("выключено")
    } else if input.display_off {
        Some("сон экрана")
    } else if input.locked {
        Some("блокировка")
    } else if input.pause_in_editors && input.editor_in_front {
        Some("монтаж")
    } else if input.fullscreen_in_front {
        Some("полный экран")
    } else {
        None
    }
}

const EDITORS: [&str; 3] = ["Resolve.exe", "Adobe Premiere Pro.exe", "AfterFX.exe"];

/// Имя файла процесса сравнивается целиком, регистр в Windows не важен.
pub fn is_editor_exe(path: &str) -> bool {
    let name = path.rsplit(['\\', '/']).next().unwrap_or(path);
    EDITORS.iter().any(|editor| editor.eq_ignore_ascii_case(name))
}

/// Частота кадров и густота с учётом экономии.
pub fn power_plan(intensity: f32, saving: bool, base_fps: f32) -> (f32, f32) {
    if saving {
        (base_fps.min(30.0), intensity * 0.5)
    } else {
        (base_fps, intensity)
    }
}

/// Прямоугольник `[left, top, right, bottom]` закрывает монитор целиком.
pub fn covers_monitor(window: [i32; 4], monitor: [i32; 4]) -> bool {
    window[0] <= monitor[0] && window[1] <= monitor[1] && window[2] >= monitor[2] && window[3] >= monitor[3]
}

/// Окна самой оболочки и наши: полным экраном не считаются.
pub fn is_shell_class(class: &str) -> bool {
    matches!(
        class,
        "Progman" | "WorkerW" | "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "XamlExplorerHostIslandWindow" | "MultitaskingViewFrame" | "SugrobOverlay" | "SugrobTray"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running() -> PauseInput {
        PauseInput { enabled: true, pause_in_editors: true, ..Default::default() }
    }

    #[test]
    fn runs_when_nothing_blocks() {
        assert_eq!(pause_reason(&running()), None);
    }

    #[test]
    fn each_reason_pauses_in_mac_order() {
        let mut input = running();
        input.fullscreen_in_front = true;
        assert_eq!(pause_reason(&input), Some("полный экран"));
        input.editor_in_front = true;
        assert_eq!(pause_reason(&input), Some("монтаж"));
        input.locked = true;
        assert_eq!(pause_reason(&input), Some("блокировка"));
        input.display_off = true;
        assert_eq!(pause_reason(&input), Some("сон экрана"));
        input.enabled = false;
        assert_eq!(pause_reason(&input), Some("выключено"));
    }

    #[test]
    fn editor_needs_the_setting() {
        let input = PauseInput { pause_in_editors: false, editor_in_front: true, ..running() };
        assert_eq!(pause_reason(&input), None);
    }

    #[test]
    fn editors_match_by_whole_file_name() {
        assert!(is_editor_exe(r"C:\Program Files\Blackmagic Design\DaVinci Resolve\Resolve.exe"));
        assert!(is_editor_exe(r"C:\Adobe\ADOBE PREMIERE PRO.EXE"));
        assert!(is_editor_exe("AfterFX.exe"));
        assert!(!is_editor_exe(r"C:\Tools\NotResolve.exe"));
        assert!(!is_editor_exe(r"C:\Resolve.exe\notepad.exe"));
    }

    #[test]
    fn saving_halves_density_and_caps_fps() {
        assert_eq!(power_plan(1.5, false, 60.0), (60.0, 1.5));
        assert_eq!(power_plan(1.5, true, 60.0), (30.0, 0.75));
        assert_eq!(power_plan(1.0, true, 24.0), (24.0, 0.5));
    }

    #[test]
    fn cover_needs_every_edge() {
        let monitor = [0, 0, 2560, 1440];
        assert!(covers_monitor(monitor, monitor));
        assert!(covers_monitor([-8, -8, 2568, 1448], monitor));
        assert!(!covers_monitor([0, 0, 2560, 1392], monitor));
        assert!(!covers_monitor([2560, 0, 5120, 1440], monitor));
    }

    #[test]
    fn shell_windows_are_excluded() {
        assert!(is_shell_class("Progman"));
        assert!(is_shell_class("WorkerW"));
        assert!(!is_shell_class("Chrome_WidgetWin_1"));
    }
}
