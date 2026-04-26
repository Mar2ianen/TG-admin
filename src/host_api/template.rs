use std::collections::HashMap;

pub fn render_template(template: &str, vars: HashMap<String, String>) -> String {
    let mut rendered = template.to_owned();
    for (key, val) in vars {
        rendered = rendered.replace(&format!("{{{}}}", key), &val);
    }
    rendered
}
