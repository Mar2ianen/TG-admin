use std::fs::File;
use std::io::BufReader;
use std::rc::Rc;
use std::time::Instant;
use telegram_moderation_os::ingress::IngressPipeline;
use telegram_moderation_os::ingress::json_export::{JsonExportAdapter, TelegramExport};
use telegram_moderation_os::router::ExecutionRouter;
use telegram_moderation_os::storage::Storage;
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

    // Setup temporary storage for counters
    let dir = tempfile::tempdir().expect("tempdir");
    let storage_path = dir.path().join("perf_test.sqlite3");
    let storage = Storage::new(storage_path).init().expect("storage init");

    let router = Rc::new(ExecutionRouter::new(0, false).with_registry(UnitRegistry::new()));
    let _pipeline = IngressPipeline::new(
        teloxide_core::Bot::new("12345:TOKEN"),
        storage.clone(),
        router.clone(),
    );

    println!("Processing events through pipeline (including counters)...");
    let start_process = Instant::now();
    let mut total_events = 0;
    for event in &events {
        total_events += 1;
        // We use a simplified process_event for perf test to avoid unnecessary overhead
        // but still trigger counters
        if let (Some(chat), Some(sender)) = (&event.chat, &event.sender) {
            if event.message.is_some() {
                storage
                    .increment_message_counters(chat.id, sender.id)
                    .expect("increment failed");
            }
        }
    }
    let process_duration = start_process.elapsed();

    println!("Processed all events in {:?}", process_duration);
    if total_events > 0 {
        println!(
            "Average processing time: {:?}",
            process_duration / total_events
        );
    }

    // Measure top participants
    let db = rusqlite::Connection::open(dir.path().join("perf_test.sqlite3")).unwrap();
    let mut stmt = db
        .prepare("SELECT user_id, count FROM message_counters ORDER BY count DESC LIMIT 10")
        .unwrap();
    let top_users: Vec<(i64, i64)> = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();

    println!("Top 10 participants by message count:");
    for (user_id, count) in top_users {
        println!("User {}: {} messages", user_id, count);
    }

    assert!(!events.is_empty(), "Should have processed some messages");
}
