import Foundation

/// Параметры запуска для замеров и отладки, задаются через окружение.
enum Config {
    private static let env = ProcessInfo.processInfo.environment

    /// Частота кадров падающего снега. Каждый кадр оверлея заставляет систему пересобрать весь экран,
    /// поэтому в покое 30: на медленном снегопаде разница с 60 почти не видна, а нагрузка вдвое меньше.
    static let fps = Float(env["SUGROB_FPS"] ?? "") ?? 30
    /// Частота, пока что-то двигается быстро: окна, снежок, сорванный снег, Дед Мороз.
    static let motionFPS = Float(env["SUGROB_MOTION_FPS"] ?? "") ?? 60
    /// Постоянная частота опроса окон. Без неё опрос адаптивный.
    static let fixedPollHz = Double(env["SUGROB_HZ"] ?? "")
    /// Рисовать рамки окон поверх снега.
    static let debugOverlay = env["SUGROB_DEBUG"] != nil
    /// Отрендерить кадр в PNG по этому пути и выйти.
    static let snapshotPath = env["SUGROB_SNAPSHOT"]
    /// В отладочном кадре показать момент срыва снега при смене Space.
    static let snapshotSlide = env["SUGROB_SNAPSHOT_SLIDE"] != nil
    /// В отладочном кадре показать ресайз окон.
    static let snapshotResize = env["SUGROB_SNAPSHOT_RESIZE"] != nil
    /// В отладочном кадре показать бросок и закрытие окна.
    static let snapshotFling = env["SUGROB_SNAPSHOT_FLING"] != nil
    /// В отладочном кадре показать перетаскивание окна вбок.
    static let snapshotDrag = env["SUGROB_SNAPSHOT_DRAG"] != nil
    /// В отладочном кадре показать стряхивание снега курсором.
    static let snapshotBrush = env["SUGROB_SNAPSHOT_BRUSH"] != nil
    /// В отладочном кадре показать снежок.
    static let snapshotBall = env["SUGROB_SNAPSHOT_BALL"] != nil
    /// В отладочном кадре показать бросок снежка.
    static let snapshotThrow = env["SUGROB_SNAPSHOT_THROW"] != nil
    /// В отладочном кадре показать крышку: `shake` — рывок, `tilt` — наклон вперёд.
    static let snapshotLid = env["SUGROB_SNAPSHOT_LID"]
    /// Ускорение сна для проверки заноса: 60 — минута сна считается за час.
    static let sheetSleepScale = Double(env["SUGROB_SHEET_SCALE"] ?? "") ?? 1
    /// В отладочном кадре показать занесённый экран: `full`, `wiped` или `shaken`.
    static let snapshotSheet = env["SUGROB_SNAPSHOT_SHEET"]
    /// В отладочном кадре показать Деда Мороза.
    static let snapshotSanta = env["SUGROB_SNAPSHOT_SANTA"] != nil
}
