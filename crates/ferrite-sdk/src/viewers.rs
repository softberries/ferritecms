//! The `viewers` setting: which signed-in users may open a plugin's admin-style pages.

/// True when `user` is listed in `setting`.
///
/// `setting` is a list of user ids separated by commas or whitespace. Blank entries are
/// ignored, ids compare exactly (case-sensitive, no prefix match), and an empty or absent
/// setting, or an absent or empty user, allows nobody.
pub fn allow(setting: Option<&str>, user: Option<&str>) -> bool {
    let (Some(setting), Some(user)) = (setting, user) else {
        return false;
    };
    !user.is_empty()
        && setting
            .split(|c: char| c == ',' || c.is_whitespace())
            .any(|id| id == user)
}

#[cfg(test)]
mod tests {
    use super::allow as viewers_allow;

    #[test]
    fn comma_and_whitespace_separate_ids() {
        let s = Some("u1, u2\nu3");
        assert!(viewers_allow(s, Some("u1")));
        assert!(viewers_allow(s, Some("u2")));
        assert!(viewers_allow(s, Some("u3")));
        assert!(!viewers_allow(s, Some("u4")));
    }

    #[test]
    fn tabs_and_runs_of_separators_are_blank_entries() {
        assert!(viewers_allow(Some(",,\t u9 ,, "), Some("u9")));
    }

    #[test]
    fn nobody_by_default() {
        assert!(!viewers_allow(None, Some("u1")));
        assert!(!viewers_allow(Some(""), Some("u1")));
        assert!(!viewers_allow(Some(" , \n"), Some("u1")));
        assert!(!viewers_allow(Some("u1"), None));
        assert!(!viewers_allow(Some("u1"), Some("")));
        assert!(!viewers_allow(Some(",u1"), Some("")));
    }

    #[test]
    fn comparison_is_exact() {
        assert!(!viewers_allow(Some("user-10"), Some("user-1")));
        assert!(!viewers_allow(Some("user-1"), Some("user-10")));
        assert!(!viewers_allow(Some("User-1"), Some("user-1")));
    }
}
