//! Выбор релиза Windows и проверка подписи. Без системных вызовов: проверяется тестами на любой системе.
use std::cmp::Ordering;

use ed25519_compact::{PublicKey, Signature};

pub const REPO: &str = "borsschec-gif/sugrob";
pub const TAG_PREFIX: &str = "win-v";
pub const EXE_ASSET: &str = "Sugrob.exe";
pub const SIG_ASSET: &str = "Sugrob.exe.sig";
const SIGN_DOMAIN: &[u8] = b"sugrob-win";

/// Только цифры и точки: тег попадает в адрес загрузки.
fn parse_version(text: &str) -> Option<Vec<u64>> {
    text.split('.').map(|part| if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) { None } else { part.parse().ok() }).collect()
}

/// Сравнение по числам, недостающие части считаются нулями: 0.10 новее 0.9, а 1.0 равно 1.0.0.
fn compare(a: &[u64], b: &[u64]) -> Ordering {
    (0..a.len().max(b.len())).map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0))).find(|o| o.is_ne()).unwrap_or(Ordering::Equal)
}

pub fn is_newer(candidate: &str, current: &str) -> bool {
    match (parse_version(candidate), parse_version(current)) {
        (Some(a), Some(b)) => compare(&a, &b).is_gt(),
        _ => false,
    }
}

#[derive(Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    pub version: String,
    pub assets: Vec<String>,
}

impl Release {
    pub fn has_asset(&self, name: &str) -> bool {
        self.assets.iter().any(|asset| asset == name)
    }

    pub fn download_path(&self, asset: &str) -> String {
        format!("/{REPO}/releases/download/{}/{asset}", self.tag)
    }
}

pub fn releases_api_path() -> String {
    format!("/repos/{REPO}/releases?per_page=100")
}

fn parse_release(item: &serde_json::Value) -> Option<Release> {
    if item["draft"].as_bool().unwrap_or(true) || item["prerelease"].as_bool().unwrap_or(true) {
        return None;
    }
    let tag = item["tag_name"].as_str()?;
    let version = tag.strip_prefix(TAG_PREFIX)?;
    parse_version(version)?;
    let assets = item["assets"].as_array()?.iter().filter_map(|asset| asset["name"].as_str().map(String::from)).collect();
    Some(Release { tag: tag.to_string(), version: version.to_string(), assets })
}

/// В списке лежат и релизы Mac: берём самый новый по номеру среди тегов `win-v`.
pub fn newest_windows_release(json: &[u8]) -> Result<Option<Release>, String> {
    let value: serde_json::Value = serde_json::from_slice(json).map_err(|error| format!("неверный JSON: {error}"))?;
    let list = value.as_array().ok_or("ответ не похож на список релизов")?;
    let mut best: Option<(Vec<u64>, Release)> = None;
    for release in list.iter().filter_map(parse_release) {
        let Some(number) = parse_version(&release.version) else { continue };
        if best.as_ref().is_none_or(|(top, _)| compare(&number, top).is_gt()) {
            best = Some((number, release));
        }
    }
    Ok(best.map(|(_, release)| release))
}

/// Заглушка в файле ключа не разбирается как ключ.
pub fn key_is_real(public_key: &[u8]) -> bool {
    PublicKey::from_slice(public_key).is_ok()
}

/// Подписывается версия вместе с файлом: старую сборку нельзя выдать за новую под чужим тегом.
pub fn signed_message(version: &str, data: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(SIGN_DOMAIN.len() + version.len() + 2 + data.len());
    message.extend_from_slice(SIGN_DOMAIN);
    message.push(0);
    message.extend_from_slice(version.as_bytes());
    message.push(0);
    message.extend_from_slice(data);
    message
}

pub fn signature_valid(public_key: &[u8], version: &str, data: &[u8], signature: &[u8]) -> bool {
    let (Ok(key), Ok(signature)) = (PublicKey::from_slice(public_key), Signature::from_slice(signature)) else {
        return false;
    };
    key.verify(signed_message(version, data), &signature).is_ok()
}

/// Обновляется только установленная копия, чтобы не заменять сборку разработчика.
pub fn is_installed_copy(exe: &str, local_app_data: &str) -> bool {
    let expected = format!("{}\\Programs\\Sugrob\\Sugrob.exe", local_app_data.trim_end_matches('\\'));
    exe.to_lowercase() == expected.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_compact::{KeyPair, Seed};

    const LIST: &str = r#"[
      {"tag_name":"v9.0.0","draft":false,"prerelease":false,"assets":[{"name":"Sugrob.zip"}]},
      {"tag_name":"win-v0.9.0","draft":false,"prerelease":false,"assets":[{"name":"Sugrob.exe"}]},
      {"tag_name":"win-v0.10.0","draft":false,"prerelease":false,"assets":[{"name":"Sugrob.exe"},{"name":"Sugrob.exe.sig"}]},
      {"tag_name":"win-v0.11.0","draft":true,"prerelease":false,"assets":[]},
      {"tag_name":"win-v0.12.0","draft":false,"prerelease":true,"assets":[]},
      {"tag_name":"win-v1.0.0-beta","draft":false,"prerelease":false,"assets":[]}
    ]"#;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0.1", "1.0"));
        assert!(!is_newer("1.0.0", "1.0"));
        assert!(!is_newer("0.9", "0.10"));
        assert!(!is_newer("abc", "0.1.0"));
        assert!(!is_newer("1.0.0-beta", "0.1.0"));
    }

    #[test]
    fn picks_newest_windows_tag_only() {
        let release = newest_windows_release(LIST.as_bytes()).ok().flatten();
        let release = release.filter(|r| r.tag == "win-v0.10.0" && r.version == "0.10.0");
        assert!(release.is_some_and(|r| r.has_asset(EXE_ASSET) && r.has_asset(SIG_ASSET) && r.download_path(EXE_ASSET) == "/borsschec-gif/sugrob/releases/download/win-v0.10.0/Sugrob.exe"));
    }

    #[test]
    fn no_windows_release_is_not_an_error() {
        assert_eq!(newest_windows_release(br#"[{"tag_name":"v1.0.0","draft":false,"prerelease":false,"assets":[]}]"#), Ok(None));
        assert!(newest_windows_release(br#"{"message":"rate limit"}"#).is_err());
        assert!(newest_windows_release(b"not json").is_err());
    }

    #[test]
    fn signature_is_checked() {
        let pair = KeyPair::from_seed(Seed::new([7; 32]));
        let signature = pair.sk.sign(signed_message("1.2.3", b"exe"), None);
        assert!(signature_valid(pair.pk.as_ref(), "1.2.3", b"exe", signature.as_ref()));
        assert!(!signature_valid(pair.pk.as_ref(), "1.2.3", b"other", signature.as_ref()));
        assert!(!signature_valid(pair.pk.as_ref(), "1.2.3", b"exe", &[0; 10]));
        let stranger = KeyPair::from_seed(Seed::new([8; 32]));
        assert!(!signature_valid(stranger.pk.as_ref(), "1.2.3", b"exe", signature.as_ref()));
    }

    #[test]
    fn signature_is_tied_to_version() {
        let pair = KeyPair::from_seed(Seed::new([7; 32]));
        let signature = pair.sk.sign(signed_message("0.1.0", b"exe"), None);
        assert!(!signature_valid(pair.pk.as_ref(), "99.0.0", b"exe", signature.as_ref()));
        // Подпись одних байтов файла больше не годится.
        let bare = pair.sk.sign(b"exe", None);
        assert!(!signature_valid(pair.pk.as_ref(), "0.1.0", b"exe", bare.as_ref()));
    }

    #[test]
    fn placeholder_is_not_a_key() {
        let placeholder = b"placeholder: update public key is not set\n";
        assert!(!key_is_real(placeholder));
        assert!(!signature_valid(placeholder, "1.0.0", b"exe", &[0; 64]));
    }

    #[test]
    fn only_the_installed_copy_updates() {
        let base = r"C:\Users\Руслан\AppData\Local";
        assert!(is_installed_copy(r"c:\users\РУСЛАН\appdata\local\Programs\Sugrob\Sugrob.exe", base));
        assert!(is_installed_copy(r"C:\Users\Руслан\AppData\Local\Programs\Sugrob\Sugrob.exe", &format!("{base}\\")));
        assert!(!is_installed_copy(r"C:\work\target\release\Sugrob.exe", base));
        assert!(!is_installed_copy(r"C:\Users\Руслан\AppData\Local\Programs\Sugrob\Other.exe", base));
    }
}
