use super::*;
use serde_json::Value;
#[test]
fn collation_and_stable_lowercase_sort_match_node_icu_across_twenty_locales() {
    let oracle: Value = serde_json::from_str(include_str!("locale_oracle.json")).unwrap();
    for case in oracle["sorts"].as_array().unwrap() {
        let locale = case["locale"].as_str().unwrap();
        let comparator = LocaleComparator::new(locale).unwrap();
        let entries = case["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        let expected: Vec<_> = case["sorted"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            comparator.sort_case_insensitive(entries),
            expected,
            "{locale}"
        );
    }
    for case in oracle["pairs"].as_array().unwrap() {
        let comparator = LocaleComparator::new(case["locale"].as_str().unwrap()).unwrap();
        let sign = match comparator.compare(
            case["left"].as_str().unwrap(),
            case["right"].as_str().unwrap(),
        ) {
            Ordering::Less => -1,
            Ordering::Equal => 0,
            Ordering::Greater => 1,
        };
        assert_eq!(sign, case["sign"].as_i64().unwrap(), "{case}");
    }
}
#[test]
fn native_locale_uses_regional_settings_and_normalizes_posix_forms() {
    assert!(LocaleComparator::new(default_sort_locale()).is_ok());
    assert!(LocaleComparator::new("???").is_err());
    for (input, expected) in [
        ("C", "en-US"),
        ("C.UTF-8", "en-US"),
        ("POSIX", "en-US"),
        ("", "en-US"),
        ("en_US_POSIX", "en-US"),
        ("zh_CN.UTF-8", "zh-CN"),
        ("sv_SE", "sv-SE"),
        ("de-DE-u-co-phonebk", "de-DE-u-co-phonebk"),
        ("sr_RS@latin", "sr-Latn-RS"),
        ("sr_RS.UTF-8@cyrillic", "sr-Cyrl-RS"),
        ("no_NO@nynorsk", "nn-NO"),
    ] {
        assert_eq!(normalize_system_locale(input), expected);
    }
}
