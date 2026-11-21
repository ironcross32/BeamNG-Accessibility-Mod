//! Moved out of `src/choices.rs`: see build.rs for why this crate's tests are integration tests.

use beam_app::choices::*;
use serde_json::json;

#[test]
fn shapes() {
    let c = ui_choices();
    assert_eq!(c["callout_intervals"][0], json!({"value": 5, "label": "5 seconds"}));
    assert_eq!(c["speed_intervals"]["labels"]["metric"][1], "Every 50 km/h");
    assert_eq!(c["gear_octaves"][0], json!({"value": 2, "label": "2"}));
    let openai = &c["ai_providers"][1];
    assert_eq!(openai["id"], "openai");
    assert!(openai["extras"][1]["values"].is_array());
    assert!(openai["extras"][0].get("values").is_none());
}
