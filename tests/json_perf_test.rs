use std::fs::File;
use std::io::BufReader;
use std::time::Instant;
use telegram_moderation_os::ingress::json_export::{JsonExportAdapter, TelegramExport};
use telegram_moderation_os::router::ExecutionRouter;
use telegram_moderation_os::unit::UnitRegistry;

#[tokio::test]
async fn test_json_export_performance() {
    let path = "/home/arch/Документы/Teloxide/Json test/result.json";
    let file = File::open(path).expect("failed to open result.json");
    let reader = BufReader::new(file);

    println!("Loading JSON...");
    let start_load = Instant::now();
    let export: TelegramExport = serde_json::from_reader(reader).expect("failed to parse JSON");
    let load_duration = start_load.elapsed();
    println!(
        "Loaded {} messages in {:?}",
        export.messages.len(),
        load_duration
    );

    let adapter = JsonExportAdapter::new();
    let start_convert = Instant::now();
    let events = adapter
        .convert_export(export)
        .expect("failed to convert export");
    let convert_duration = start_convert.elapsed();
    println!(
        "Converted to {} EventContexts in {:?}",
        events.len(),
        convert_duration
    );

    let mut manifest = telegram_moderation_os::unit::UnitManifest::new(
        telegram_moderation_os::unit::UnitDefinition::new("perf.test"),
        telegram_moderation_os::unit::TriggerSpec::EventType {
            events: vec![telegram_moderation_os::unit::UnitEventType::Message],
        },
        telegram_moderation_os::unit::ServiceSpec::new("echo"),
    );
    let registry = UnitRegistry::load_manifests(vec![manifest]).registry;
    let router = ExecutionRouter::new().with_registry(registry);

    println!("Routing messages...");
    let start_route = Instant::now();
    let mut routed_count = 0;
    for event in &events {
        let plan = router.plan(event);
        if !plan.lanes.is_empty() {
            routed_count += 1;
        }
    }
    let route_duration = start_route.elapsed();

    println!(
        "Routed {}/{} messages in {:?}",
        routed_count,
        events.len(),
        route_duration
    );
    println!(
        "Average routing time (plan): {:?}",
        route_duration / events.len() as u32
    );

    assert!(!events.is_empty(), "Should have processed some messages");
}
