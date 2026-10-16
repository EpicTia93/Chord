//! VoiceOver can use Caps Lock as its modifier key, but it only recognizes the physical
//! Caps key, which Chord remaps while it owns input (see `caps_remap`). Chord behaves
//! the same for everyone and only warns VoiceOver users to use Control-Option instead.

use objc2_core_foundation::{
    CFNumber, CFPreferencesAppSynchronize, CFPreferencesCopyAppValue, CFPreferencesCopyKeyList,
    CFString, kCFPreferencesAnyHost, kCFPreferencesCurrentUser,
};

/// VoiceOver's settings; it creates them the first time it runs.
const DOMAIN: &str = "com.apple.VoiceOver4/default";
const MODIFIER_KEY: &str = "SCRKeysToUseForVOModifier";
/// `SCRVOModifierControlOption`. 1 is Caps Lock and 2 Control-Option or Caps Lock, which
/// is also the default when the key is missing.
const CONTROL_OPTION: i64 = 0;

pub const UTILITY_BUNDLE_ID: &str = "com.apple.VoiceOverUtility";

/// Whether this user has run VoiceOver with Caps Lock as a possible modifier. Read on
/// demand: VoiceOver doesn't announce changes to its settings.
pub fn uses_caps_lock_modifier() -> bool {
    reads_caps_lock_modifier(&CFString::from_static_str(DOMAIN))
}

fn reads_caps_lock_modifier(domain: &CFString) -> bool {
    CFPreferencesAppSynchronize(domain);
    // SAFETY: reading Core Foundation constants.
    let (user, host) = unsafe { (kCFPreferencesCurrentUser, kCFPreferencesAnyHost) };
    let used = CFPreferencesCopyKeyList(domain, user, host).is_some_and(|keys| keys.count() > 0);
    let modifier = CFPreferencesCopyAppValue(&CFString::from_static_str(MODIFIER_KEY), domain)
        .and_then(|value| value.downcast_ref::<CFNumber>().and_then(CFNumber::as_i64));
    modifier_includes_caps_lock(used, modifier)
}

fn modifier_includes_caps_lock(used: bool, modifier: Option<i64>) -> bool {
    used && modifier != Some(CONTROL_OPTION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_voice_over_users_with_caps_lock_as_a_modifier_are_warned() {
        // Never ran VoiceOver: its default would include Caps Lock, but it doesn't matter.
        assert!(!modifier_includes_caps_lock(false, None));
        assert!(modifier_includes_caps_lock(true, None));
        assert!(modifier_includes_caps_lock(true, Some(1)));
        assert!(modifier_includes_caps_lock(true, Some(2)));
        assert!(!modifier_includes_caps_lock(true, Some(CONTROL_OPTION)));
    }

    #[test]
    #[ignore = "reads this user's VoiceOver settings"]
    fn reads_voice_over_settings() {
        println!("uses Caps Lock modifier: {}", uses_caps_lock_modifier());
    }

    #[test]
    #[ignore = "writes, then deletes, a scratch preferences domain for this user"]
    fn reads_the_modifier_from_preferences() {
        use objc2_core_foundation::{CFPreferencesSetAppValue, CFPropertyList};
        let name = format!("dev.chord.voice-over-test-{}/default", std::process::id());
        let domain = CFString::from_str(&name);
        let key = CFString::from_static_str(MODIFIER_KEY);
        let set = |value: Option<&CFPropertyList>| {
            // SAFETY: a property list value (or none) for a string key.
            unsafe { CFPreferencesSetAppValue(&key, value, &domain) };
            assert!(CFPreferencesAppSynchronize(&domain));
        };
        assert!(!reads_caps_lock_modifier(&domain));
        for (value, expected) in [(1, true), (2, true), (CONTROL_OPTION, false)] {
            set(Some(&CFNumber::new_i64(value)));
            assert_eq!(reads_caps_lock_modifier(&domain), expected, "value {value}");
        }
        set(None);
        let home = std::env::var("HOME").unwrap();
        let _ = std::fs::remove_file(format!("{home}/Library/Preferences/{name}.plist"));
        let _ = std::fs::remove_dir(format!(
            "{home}/Library/Preferences/{}",
            name.trim_end_matches("/default")
        ));
    }
}
