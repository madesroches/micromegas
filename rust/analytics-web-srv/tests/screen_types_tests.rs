//! Unit tests for screen_types module

use analytics_web_srv::screen_types::ScreenType;

#[test]
fn test_screen_type_serialization() {
    assert_eq!(
        serde_json::to_string(&ScreenType::ProcessList).expect("serialize"),
        "\"process_list\""
    );
    assert_eq!(
        serde_json::to_string(&ScreenType::Metrics).expect("serialize"),
        "\"metrics\""
    );
    assert_eq!(
        serde_json::to_string(&ScreenType::Log).expect("serialize"),
        "\"log\""
    );
}

#[test]
fn test_screen_type_deserialization() {
    assert_eq!(
        serde_json::from_str::<ScreenType>("\"process_list\"").expect("deserialize"),
        ScreenType::ProcessList
    );
    assert_eq!(
        serde_json::from_str::<ScreenType>("\"metrics\"").expect("deserialize"),
        ScreenType::Metrics
    );
    assert_eq!(
        serde_json::from_str::<ScreenType>("\"log\"").expect("deserialize"),
        ScreenType::Log
    );
}

#[test]
fn test_screen_type_from_str() {
    assert_eq!(
        "process_list".parse::<ScreenType>().unwrap(),
        ScreenType::ProcessList
    );
    assert_eq!(
        "metrics".parse::<ScreenType>().unwrap(),
        ScreenType::Metrics
    );
    assert_eq!("log".parse::<ScreenType>().unwrap(), ScreenType::Log);
    assert!("invalid".parse::<ScreenType>().is_err());
}

#[test]
fn test_all_screen_types() {
    let all = ScreenType::all();
    assert_eq!(all.len(), 3);
    assert!(all.contains(&ScreenType::ProcessList));
    assert!(all.contains(&ScreenType::Metrics));
    assert!(all.contains(&ScreenType::Log));
}

#[test]
fn test_screen_type_as_str() {
    assert_eq!(ScreenType::ProcessList.as_str(), "process_list");
    assert_eq!(ScreenType::Metrics.as_str(), "metrics");
    assert_eq!(ScreenType::Log.as_str(), "log");
}

#[test]
fn test_screen_type_info() {
    let process_info = ScreenType::ProcessList.info();
    assert_eq!(process_info.name, "process_list");
    assert!(!process_info.icon.is_empty());
    assert!(!process_info.description.is_empty());

    let metrics_info = ScreenType::Metrics.info();
    assert_eq!(metrics_info.name, "metrics");
    assert!(!metrics_info.icon.is_empty());
    assert!(!metrics_info.description.is_empty());

    let log_info = ScreenType::Log.info();
    assert_eq!(log_info.name, "log");
    assert!(!log_info.icon.is_empty());
    assert!(!log_info.description.is_empty());
}

#[test]
fn test_screen_type_default_config() {
    // ProcessList config should have sql field
    let process_config = ScreenType::ProcessList.default_config();
    assert!(process_config.get("sql").is_some());
    assert!(
        process_config["sql"]
            .as_str()
            .unwrap()
            .contains("processes")
    );

    // Metrics config should have sql field
    let metrics_config = ScreenType::Metrics.default_config();
    assert!(metrics_config.get("sql").is_some());
    assert!(metrics_config["sql"].as_str().unwrap().contains("measures"));

    // Log config should have sql field
    let log_config = ScreenType::Log.default_config();
    assert!(log_config.get("sql").is_some());
    assert!(log_config["sql"].as_str().unwrap().contains("log_entries"));
}

#[test]
fn test_screen_type_from_str_error() {
    let err = "unknown_type".parse::<ScreenType>().unwrap_err();
    assert!(err.to_string().contains("unknown_type"));
    assert!(err.to_string().contains("process_list"));
}

#[test]
fn test_screen_type_roundtrip() {
    // Test that as_str -> from_str -> as_str is consistent
    for screen_type in ScreenType::all() {
        let s = screen_type.as_str();
        let parsed: ScreenType = s.parse().expect("should parse");
        assert_eq!(screen_type, parsed);
        assert_eq!(s, parsed.as_str());
    }
}

#[test]
fn test_screen_type_serde_roundtrip() {
    for screen_type in ScreenType::all() {
        let json = serde_json::to_string(&screen_type).expect("serialize");
        let parsed: ScreenType = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(screen_type, parsed);
    }
}
