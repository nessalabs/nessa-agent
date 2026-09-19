use super::*;

#[test]
fn a_reserved_device_is_one_whatever_follows_the_first_dot() {
    for reserved in ["con", "nul", "aux", "prn", "com1", "com9", "lpt1", "lpt9"] {
        assert!(names_a_device(reserved), "{reserved:?} is a device");
        assert!(
            names_a_device(&format!("{reserved}.txt")),
            "{reserved:?} with an extension is still a device"
        );
        assert!(
            names_a_device(&format!("{reserved}.1.0.0")),
            "{reserved:?} with several is still a device"
        );
    }
}

#[test]
fn an_ordinary_name_is_not_a_device() {
    for ordinary in [
        "opencode", "claude", "codex", "1.18.31", "console", "com", "com10", "lpt0", "nullify", "",
    ] {
        assert!(!names_a_device(ordinary), "{ordinary:?} is not a device");
    }
}
