mod tests_db {
    include!("tests_db.rs");
}

#[path = "tests_ctx.rs"]
mod tests_ctx;

#[path = "tests_audit.rs"]
mod tests_audit;

#[path = "tests_history.rs"]
mod tests_history;

#[path = "tests_surface.rs"]
mod tests_surface;

#[path = "tests_unit_status.rs"]
mod tests_unit_status;

#[path = "tests_capabilities.rs"]
mod tests_capabilities;

#[path = "tests_template.rs"]
mod tests_template;
