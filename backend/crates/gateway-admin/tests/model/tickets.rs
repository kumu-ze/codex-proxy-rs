use gateway_admin::model::tickets::{TicketAccountPolicy, TicketMode, TicketSettings};

#[test]
fn ticket_plan_defaults_and_explicit_override_are_distinct() {
    let mut settings = TicketSettings::default();
    assert!(!settings.enabled);
    assert!(!settings.inject);
    assert_eq!(settings.manual_interval_seconds, 0);
    assert_eq!(settings.target_length("acct_a", Some("plus")), 292);
    assert_eq!(settings.target_length("acct_a", Some("pro")), 292);
    assert_eq!(settings.target_length("acct_a", Some("business")), 332);
    assert_eq!(settings.target_length("acct_a", Some("TEAM")), 332);
    settings.accounts.insert(
        "acct_a".into(),
        TicketAccountPolicy {
            mode: TicketMode::Manual,
            target_length: Some(312),
        },
    );
    assert_eq!(settings.target_length("acct_a", Some("pro")), 312);
    assert_eq!(settings.target_length("acct_b", Some("pro")), 292);
    assert!(settings.validate());
}

#[test]
fn ticket_settings_reject_unbounded_or_inconsistent_work() {
    let base = TicketSettings::default();
    for settings in [
        TicketSettings {
            interval_seconds: 0,
            ..base.clone()
        },
        TicketSettings {
            manual_interval_seconds: 86401,
            ..base.clone()
        },
        TicketSettings {
            ttl_seconds: 86400,
            ..base.clone()
        },
        TicketSettings {
            refresh_before_seconds: 3600,
            ..base.clone()
        },
        TicketSettings {
            plus_pro_length: 0,
            ..base.clone()
        },
        TicketSettings {
            models: vec![],
            ..base.clone()
        },
        TicketSettings {
            models: vec!["gpt-6-astra".into(), "gpt-6-astra".into()],
            ..base.clone()
        },
        TicketSettings {
            models: vec!["model\r\nheader".into()],
            ..base
        },
    ] {
        assert!(!settings.validate());
    }
}
