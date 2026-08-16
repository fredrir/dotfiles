//! Pure validators shared by the M2 schema lowerers.
//!
//! These functions validate decoded values. Syntax-level concerns such as
//! whether a value was quoted or interpolated remain the lowerer's job.

use dotfile_source::RepoPath;
use unicode_normalization::UnicodeNormalization;

pub(crate) const MANAGERS: &[&str] = &["brew", "pacman", "apt"];
pub(crate) const INSTALLERS: &[&str] = &[
    "brew-formula",
    "brew-cask",
    "pacman",
    "aur",
    "apt",
    "cargo",
    "uv",
];

/// Returns whether `value` is already in Unicode Normalization Form C.
pub(crate) fn is_nfc(value: &str) -> bool {
    value.nfc().eq(value.chars())
}

/// Validates recipient labels from section 26.1.
pub(crate) fn is_label(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// Validates host extension fact keys.
pub(crate) fn is_extension_key(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_uppercase())
        && bytes.all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}

/// Validates an executable basename accepted by the command adapter.
pub(crate) fn is_command_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'+' | b'-'))
}

/// Validates a canonical four-digit permission mode with no special bits.
pub(crate) fn is_mode(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 4
        && bytes[0] == b'0'
        && bytes[1..].iter().all(|byte| matches!(byte, b'0'..=b'7'))
}

/// Checks a decoded string against a closed string enum.
pub(crate) fn is_enum_value(value: &str, allowed: &[&str]) -> bool {
    allowed.contains(&value)
}

/// Validates a normalized repository-relative group directory.
pub(crate) fn is_repository_directory(value: &str) -> bool {
    is_nfc(value) && RepoPath::new(value).is_ok()
}

/// Validates a resolved machine path from section 6.6.
///
/// The filesystem root is a valid observation path. It is kept separate from
/// [`is_destination_path`], where `/` cannot name a leaf destination.
pub(crate) fn is_machine_path(value: &str) -> bool {
    is_nfc(value) && is_symbolic_or_absolute_path(value, true)
}

/// Validates a resolved destination path from section 6.6.
pub(crate) fn is_destination_path(value: &str) -> bool {
    is_nfc(value) && is_symbolic_or_absolute_path(value, false)
}

fn is_symbolic_or_absolute_path(value: &str, allow_absolute_root: bool) -> bool {
    if value.is_empty()
        || value.contains('\\')
        || value.contains("$HOME")
        || value.chars().any(char::is_control)
    {
        return false;
    }

    let remainder = if value == "~" {
        return true;
    } else if let Some(remainder) = value.strip_prefix("~/") {
        remainder
    } else if value == "/" {
        return allow_absolute_root;
    } else if let Some(remainder) = value.strip_prefix('/') {
        // This also rejects a leading `//` through the empty-component check.
        remainder
    } else {
        return false;
    };

    !remainder.is_empty()
        && !remainder.ends_with('/')
        && !remainder.contains('~')
        && remainder
            .split('/')
            .all(|component| !matches!(component, "" | "." | ".."))
}

/// Validates a canonical public X25519 age recipient.
///
/// Native X25519 recipients are the Bech32 encoding of exactly 32 bytes with
/// the lowercase HRP `age`. Requiring that exact HRP and length also excludes
/// private identities and other age recipient families.
pub(crate) fn is_age_public_recipient(value: &str) -> bool {
    // `age` + separator + ceil(32 * 8 / 5) data symbols + six checksum symbols.
    if value.len() != 62 || !value.starts_with("age1") {
        return false;
    }

    let mut checksum = 1_u32;
    for symbol in [3_u8, 3, 3, 0, 1, 7, 5] {
        checksum = bech32_polymod_step(checksum, symbol);
    }

    let mut last_payload_symbol = 0;
    for (index, byte) in value.bytes().skip(4).enumerate() {
        let Some(symbol) = bech32_symbol(byte) else {
            return false;
        };
        if index == 51 {
            last_payload_symbol = symbol;
        }
        checksum = bech32_polymod_step(checksum, symbol);
    }

    // The 256-bit payload leaves one data bit in the final five-bit symbol;
    // the remaining four bits are canonical zero padding.
    checksum == 1 && last_payload_symbol & 0x0f == 0
}

fn bech32_symbol(byte: u8) -> Option<u8> {
    const CHARSET: &[u8; 32] = b"qpzry9x8gf2tvdw0s3jn54khce6mua7l";
    CHARSET
        .iter()
        .position(|candidate| *candidate == byte)
        .map(|index| index as u8)
}

fn bech32_polymod_step(checksum: u32, value: u8) -> u32 {
    const GENERATORS: [u32; 5] = [
        0x3b6a_57b2,
        0x2650_8e6d,
        0x1ea1_19fa,
        0x3d42_33dd,
        0x2a14_62b3,
    ];

    let top = checksum >> 25;
    let mut next = ((checksum & 0x01ff_ffff) << 5) ^ u32::from(value);
    for (bit, generator) in GENERATORS.into_iter().enumerate() {
        if (top >> bit) & 1 != 0 {
            next ^= generator;
        }
    }
    next
}

/// Validates the deliberately small repository-relative glob grammar in
/// section 26.2.
pub(crate) fn is_scan_glob(value: &str) -> bool {
    if !is_nfc(value)
        || value.is_empty()
        || value.starts_with('/')
        || value.ends_with('/')
        || value.starts_with('!')
        || value
            .chars()
            .any(|scalar| scalar.is_control() || matches!(scalar, '\\' | '[' | ']' | '{' | '}'))
    {
        return false;
    }

    value.split('/').all(|component| {
        if matches!(component, "" | "." | "..") {
            return false;
        }
        if component == "**" {
            return true;
        }

        // Two adjacent stars have meaning only as the complete `**`
        // component. Separate single-star wildcards remain valid.
        !component.as_bytes().windows(2).any(|pair| pair == b"**")
    })
}

/// Validates an eight-digit lowercase benchmark epoch.
pub(crate) fn is_benchmark_epoch(value: &str) -> bool {
    value.len() == 8
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// Returns the epoch suffix of a syntactically valid benchmark run ID.
pub(crate) fn benchmark_run_id_epoch(run_id: &str) -> Option<&str> {
    let bytes = run_id.as_bytes();
    if bytes.len() != 29
        || ![0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
            .into_iter()
            .all(|index| bytes[index].is_ascii_digit())
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b'-'
        || bytes[16] != b'-'
        || bytes[19] != b'Z'
        || bytes[20] != b'-'
    {
        return None;
    }

    let epoch = &run_id[21..];
    is_benchmark_epoch(epoch).then_some(epoch)
}

/// Validates both the run-ID shape and its correlation with the epoch key.
pub(crate) fn is_benchmark_run_id_for_epoch(run_id: &str, epoch: &str) -> bool {
    is_benchmark_epoch(epoch) && benchmark_run_id_epoch(run_id) == Some(epoch)
}

/// Returns whether a decoded string contains no Unicode line-break scalar.
pub(crate) fn is_one_line(value: &str) -> bool {
    !value.chars().any(|scalar| {
        matches!(
            scalar,
            '\n' | '\r' | '\u{000b}' | '\u{000c}' | '\u{0085}' | '\u{2028}' | '\u{2029}'
        )
    })
}

pub(crate) fn is_manager(value: &str) -> bool {
    is_enum_value(value, MANAGERS)
}

pub(crate) fn is_installer(value: &str) -> bool {
    is_enum_value(value, INSTALLERS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nfc_is_checked_without_normalizing() {
        assert!(is_nfc("caf\u{e9}"));
        assert!(!is_nfc("cafe\u{301}"));
    }

    #[test]
    fn ascii_name_shapes_are_distinct() {
        for value in ["archpc", "recovery2", "x.y-z_9"] {
            assert!(is_label(value), "label {value:?}");
        }
        for value in ["", ".", "_name", "name+extra", "caf\u{e9}"] {
            assert!(!is_label(value), "label {value:?}");
        }

        for value in ["CPU", "CPU_COOLER", "MEMORY2"] {
            assert!(is_extension_key(value), "extension key {value:?}");
        }
        for value in ["", "2CPU", "Cpu", "CPU-COOLER", "\u{c5}"] {
            assert!(!is_extension_key(value), "extension key {value:?}");
        }

        for value in ["7z", "fc-cache", "wezterm.mux", "c++"] {
            assert!(is_command_name(value), "command {value:?}");
        }
        for value in ["", ".", "..", ".hidden", "a/b", " two", "two ", "caf\u{e9}"] {
            assert!(!is_command_name(value), "command {value:?}");
        }
    }

    #[test]
    fn mode_excludes_special_bits_and_noncanonical_spellings() {
        for value in ["0000", "0600", "0644", "0700", "0755", "0777"] {
            assert!(is_mode(value), "mode {value:?}");
        }
        for value in ["644", "00644", "0800", "1755", "4755", "0o644", "06a4"] {
            assert!(!is_mode(value), "mode {value:?}");
        }
    }

    #[test]
    fn enum_membership_is_exact() {
        assert!(is_enum_value("copy", &["link", "copy", "none"]));
        assert!(!is_enum_value("COPY", &["link", "copy", "none"]));
        assert!(!is_enum_value("", &[]));
    }

    #[test]
    fn repository_directories_are_strict_relative_nfc_paths() {
        for value in ["shared", "linux/arch", "caf\u{e9}/tools", "foo..bar"] {
            assert!(is_repository_directory(value), "directory {value:?}");
        }
        for value in [
            "",
            "/shared",
            "shared/",
            "a//b",
            "./shared",
            "a/../b",
            "a\\b",
            "a\n",
            "cafe\u{301}",
        ] {
            assert!(!is_repository_directory(value), "directory {value:?}");
        }
    }

    #[test]
    fn machine_and_destination_paths_follow_section_6_6() {
        for value in ["~", "~/.config/tool", "/etc/tool", "/caf\u{e9}/tool"] {
            assert!(is_machine_path(value), "machine path {value:?}");
            assert!(is_destination_path(value), "destination path {value:?}");
        }
        assert!(is_machine_path("/"));
        assert!(!is_destination_path("/"));
        assert!(!is_machine_path("/cafe\u{301}"));
        assert!(!is_destination_path("/cafe\u{301}"));

        for value in [
            "",
            "relative/path",
            "~user/file",
            "$HOME/file",
            "~/a/$HOME",
            "//server/share",
            "~/",
            "/etc/",
            "~/a//b",
            "/a/./b",
            "/a/../b",
            "/a/~b",
            "/a\\b",
            "/a\nb",
        ] {
            assert!(!is_machine_path(value), "machine path {value:?}");
            assert!(!is_destination_path(value), "destination path {value:?}");
        }
    }

    #[test]
    fn age_recipients_require_x25519_shape_padding_and_checksum() {
        for value in [
            "age15wjewk6yjs5vsezah0sa9vz3gyl569eexwj74l8dvrc2vlsxuq3q7hq52d",
            "age1535gunzf00mxeww9v0ccerj5ygcthngm353hha8grwjqgh723qts99a3jm",
            "age1wflp6cynwm97wndq5zxmpaxwz59h62a7dku8qdyue5zm9g4djfnqwj9n0m",
        ] {
            assert!(is_age_public_recipient(value), "recipient {value:?}");
        }

        for value in [
            "",
            "AGE15WJEWK6YJS5VSEZAH0SA9VZ3GYL569EEXWJ74L8DVRC2VLSXUQ3Q7HQ52D",
            "AGE-SECRET-KEY-1KTYK6RVLN5TAPE7VF6FQQSKZ9HWWCDSKUGXXNUQDWZ7XXT5YK5LSF3UTKQ",
            "age15wjewk6yjs5vsezah0sa9vz3gyl569eexwj74l8dvrc2vlsxuq3q7hq52q",
            "age15wjewk6yjs5vsezah0sa9vz3gyl569eexwj74l8dvrc2vlsxuq3q7hq52",
            "age15wjewk6yjs5vsezah0sa9vz3gyl569eexwj74l8dvrc2vlsxuq3q7hq52di",
        ] {
            assert!(!is_age_public_recipient(value), "recipient {value:?}");
        }
    }

    #[test]
    fn scan_globs_accept_only_the_v1_grammar() {
        for value in [
            "scripts/python/tests/transcript/test_redact.py",
            "shared/obsidian/plugins/**",
            "**/secrets/*.yaml",
            "a*/b?/c*d",
            "caf\u{e9}/?.txt",
            "bang!/file",
            "**",
        ] {
            assert!(is_scan_glob(value), "glob {value:?}");
        }

        for value in [
            "",
            "/foo",
            "foo/",
            "foo//bar",
            "./foo",
            "foo/../bar",
            "foo\\bar",
            "!secret",
            "foo/[ab]",
            "foo/{a,b}",
            "foo/ab**cd",
            "foo/**bar",
            "foo/***/bar",
            "foo\nbar",
            "cafe\u{301}/*.txt",
        ] {
            assert!(!is_scan_glob(value), "glob {value:?}");
        }
    }

    #[test]
    fn benchmark_values_have_exact_shape_and_matching_suffix() {
        assert!(is_benchmark_epoch("10db7d1f"));
        assert!(!is_benchmark_epoch("10DB7D1F"));
        assert!(!is_benchmark_epoch("10db7d1"));
        assert!(!is_benchmark_epoch("10db7d1g"));

        let run_id = "2026-08-13T11-34-32Z-10db7d1f";
        assert_eq!(benchmark_run_id_epoch(run_id), Some("10db7d1f"));
        assert!(is_benchmark_run_id_for_epoch(run_id, "10db7d1f"));
        assert!(!is_benchmark_run_id_for_epoch(run_id, "31c0c4e0"));
        assert_eq!(
            benchmark_run_id_epoch("2026-08-13T11:34:32Z-10db7d1f"),
            None
        );
        assert_eq!(
            benchmark_run_id_epoch("2026-08-13t11-34-32Z-10db7d1f"),
            None
        );
        assert_eq!(
            benchmark_run_id_epoch("2026-08-13T11-34-32Z-10DB7D1F"),
            None
        );
        // The language freezes a lexical timestamp pattern, not calendar validation.
        assert!(is_benchmark_run_id_for_epoch(
            "9999-99-99T99-99-99Z-abcdef01",
            "abcdef01"
        ));
    }

    #[test]
    fn one_line_rejects_ascii_and_unicode_line_breaks_but_not_tabs() {
        assert!(is_one_line(""));
        assert!(is_one_line("one\tline"));
        for scalar in [
            '\n', '\r', '\u{000b}', '\u{000c}', '\u{0085}', '\u{2028}', '\u{2029}',
        ] {
            assert!(!is_one_line(&format!("left{scalar}right")));
        }
    }

    #[test]
    fn manager_and_installer_registries_are_closed() {
        for value in ["brew", "pacman", "apt"] {
            assert!(is_manager(value));
        }
        for value in [
            "brew-formula",
            "brew-cask",
            "pacman",
            "aur",
            "apt",
            "cargo",
            "uv",
        ] {
            assert!(is_installer(value));
        }
        for value in ["", "BREW", "dnf", "nix"] {
            assert!(!is_manager(value));
            assert!(!is_installer(value));
        }
    }
}
