//! Compare a spec module (untyped `serde_json::Value`) against parsed code.

use std::collections::HashMap;

use serde_json::Value;

use super::construct::{CodeConstruct, ConstructKind};
use super::util::snake_case;
use super::AuditItem;

fn strip_reference(t: &str) -> &str {
    let s = t.trim();
    if let Some(rest) = s.strip_prefix("&mut ") {
        rest.trim()
    } else if let Some(rest) = s.strip_prefix('&') {
        rest.trim()
    } else {
        s
    }
}

fn translate(spec_type: &str, map: Option<&HashMap<String, String>>) -> String {
    match map {
        Some(m) => m.get(spec_type).cloned().unwrap_or_else(|| spec_type.to_string()),
        None => spec_type.to_string(),
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}

/// Produce [`AuditItem`]s describing matches, mismatches, missing items,
/// and (if `show_uncovered`) uncovered code constructs.
pub fn compare_module(
    module: &Value,
    code: &[CodeConstruct],
    show_uncovered: bool,
    type_map: Option<&HashMap<String, String>>,
) -> Vec<AuditItem> {
    let mut items = Vec::new();
    let mut matched: Vec<bool> = vec![false; code.len()];

    let Some(module_name) = str_field(module, "name") else {
        return items;
    };

    // 1) Module struct/type.
    if let Some((idx, _)) = code
        .iter()
        .enumerate()
        .find(|(_, c)| c.kind == ConstructKind::Struct && c.name == module_name)
    {
        items.push(AuditItem::Match {
            kind: "struct".into(),
            name: module_name.into(),
        });
        matched[idx] = true;
    } else {
        items.push(AuditItem::Missing {
            kind: "struct".into(),
            name: module_name.into(),
            detail: format!("struct {module_name} not found"),
        });
    }

    // 2) Actions → functions.
    if let Some(actions) = module.get("actions").and_then(Value::as_array) {
        for action in actions {
            let Some(action_name) = str_field(action, "name") else {
                continue;
            };
            let snake = snake_case(action_name);

            let found = code.iter().enumerate().find(|(_, c)| {
                c.kind == ConstructKind::Function
                    && (c.name == action_name || c.name == snake)
            });

            let spec_params = action
                .get("params")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let spec_non_self: Vec<&Value> = spec_params
                .iter()
                .filter(|p| str_field(p, "name") != Some("self"))
                .collect();

            match found {
                Some((idx, func)) => {
                    matched[idx] = true;

                    let code_non_self: Vec<_> = func
                        .parameters
                        .iter()
                        .filter(|p| p.name != "self")
                        .collect();

                    let spec_fmt: Vec<String> = spec_params
                        .iter()
                        .map(|p| {
                            let n = str_field(p, "name").unwrap_or("?");
                            let t = str_field(p, "type").unwrap_or("?");
                            let d = str_field(p, "direction").unwrap_or("in");
                            let arrow = if d == "out" { "-> " } else { "" };
                            format!("{arrow}{n}: {t}")
                        })
                        .collect();
                    let code_fmt: Vec<String> = func
                        .parameters
                        .iter()
                        .map(|p| {
                            if p.type_name.is_empty() {
                                p.name.clone()
                            } else {
                                format!("{}: {}", p.name, p.type_name)
                            }
                        })
                        .collect();

                    if spec_non_self.len() != code_non_self.len() {
                        items.push(AuditItem::Mismatch {
                            kind: "action".into(),
                            name: action_name.into(),
                            spec_detail: format!("({})", spec_fmt.join(", ")),
                            code_detail: format!("({})", code_fmt.join(", ")),
                        });
                        continue;
                    }

                    if let Some(map) = type_map {
                        let mut type_mismatches = Vec::new();
                        for (sp, cp) in spec_non_self.iter().zip(code_non_self.iter()) {
                            let sp_name = str_field(sp, "name").unwrap_or("?");
                            let sp_type = str_field(sp, "type").unwrap_or("?");
                            let translated = translate(sp_type, Some(map));
                            let code_stripped = strip_reference(&cp.type_name);
                            if translated != code_stripped {
                                type_mismatches.push(format!(
                                    "{sp_name}: spec {translated} → code {}",
                                    cp.type_name
                                ));
                            }
                        }
                        if type_mismatches.is_empty() {
                            items.push(AuditItem::Match {
                                kind: "action".into(),
                                name: action_name.into(),
                            });
                        } else {
                            items.push(AuditItem::Mismatch {
                                kind: "action".into(),
                                name: action_name.into(),
                                spec_detail: format!("({})", spec_fmt.join(", ")),
                                code_detail: format!(
                                    "type mismatches: [{}]",
                                    type_mismatches.join("; ")
                                ),
                            });
                        }
                    } else {
                        items.push(AuditItem::Match {
                            kind: "action".into(),
                            name: action_name.into(),
                        });
                    }
                }
                None => {
                    items.push(AuditItem::Missing {
                        kind: "action".into(),
                        name: action_name.into(),
                        detail: format!(
                            "fn not found (tried: {action_name}, {snake})"
                        ),
                    });
                }
            }
        }
    }

    // 3) State machines → enums.
    if let Some(fsms) = module.get("state_machines").and_then(Value::as_array) {
        for fsm in fsms {
            let Some(fsm_name) = str_field(fsm, "name") else {
                continue;
            };
            let states: Vec<String> = fsm
                .get("states")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();

            let expect_enum = format!("{module_name}State");
            let alt = format!("{}State", fsm_name.replace("FSM", ""));

            let found = code.iter().enumerate().find(|(_, c)| {
                c.kind == ConstructKind::Enum
                    && (c.name == expect_enum
                        || c.name == alt
                        || c.name.ends_with("State"))
            });

            match found {
                Some((idx, ec)) => {
                    matched[idx] = true;
                    let missing: Vec<&String> = states
                        .iter()
                        .filter(|st| {
                            !ec.variants.iter().any(|v| {
                                v.to_lowercase() == st.to_lowercase()
                                    || snake_case(v) == snake_case(st)
                            })
                        })
                        .collect();
                    if missing.is_empty() {
                        items.push(AuditItem::Match {
                            kind: "state_machine".into(),
                            name: fsm_name.into(),
                        });
                    } else {
                        items.push(AuditItem::Mismatch {
                            kind: "state_machine".into(),
                            name: fsm_name.into(),
                            spec_detail: format!("states: [{}]", states.join(", ")),
                            code_detail: format!(
                                "enum {} missing: [{}]",
                                ec.name,
                                missing
                                    .iter()
                                    .map(|s| s.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                        });
                    }
                }
                None => {
                    items.push(AuditItem::Missing {
                        kind: "state_machine".into(),
                        name: fsm_name.into(),
                        detail: format!("no state enum found (tried: {expect_enum}, {alt})"),
                    });
                }
            }
        }
    }

    // 4) Ports → traits (Rust) / structs.
    if let Some(ports) = module.get("ports").and_then(Value::as_array) {
        for port in ports {
            let Some(port_name) = str_field(port, "name") else {
                continue;
            };
            let Some(port_type) = str_field(port, "port_type") else {
                continue;
            };
            let found = code.iter().enumerate().find(|(_, c)| {
                (c.kind == ConstructKind::Trait || c.kind == ConstructKind::Struct)
                    && c.name == port_type
            });
            match found {
                Some((idx, _)) => {
                    matched[idx] = true;
                    items.push(AuditItem::Match {
                        kind: "port_type".into(),
                        name: port_type.into(),
                    });
                }
                None => {
                    items.push(AuditItem::Missing {
                        kind: "port_type".into(),
                        name: port_type.into(),
                        detail: format!(
                            "type {port_type} not found in source (for port {port_name})"
                        ),
                    });
                }
            }
        }
    }

    // 5) Uncovered code.
    if show_uncovered {
        for (idx, c) in code.iter().enumerate() {
            if matched[idx] {
                continue;
            }
            let kind = match c.kind {
                ConstructKind::Function => "function",
                ConstructKind::Struct => "struct",
                ConstructKind::Enum => "enum",
                ConstructKind::ImplBlock => "impl",
                ConstructKind::Trait => "trait",
            };
            items.push(AuditItem::Uncovered {
                kind: kind.into(),
                name: c.name.clone(),
                line: c.line,
            });
        }
    }

    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::ParsedParameter;
    use serde_json::json;

    fn construct_struct(name: &str) -> CodeConstruct {
        CodeConstruct {
            kind: ConstructKind::Struct,
            name: name.into(),
            parameters: vec![],
            fields: vec![],
            variants: vec![],
            methods: vec![],
            line: 1,
        }
    }

    fn construct_fn(name: &str, params: Vec<(&str, &str)>) -> CodeConstruct {
        CodeConstruct {
            kind: ConstructKind::Function,
            name: name.into(),
            parameters: params
                .into_iter()
                .map(|(n, t)| ParsedParameter {
                    name: n.into(),
                    type_name: t.into(),
                })
                .collect(),
            fields: vec![],
            variants: vec![],
            methods: vec![],
            line: 10,
        }
    }

    fn construct_enum(name: &str, variants: &[&str]) -> CodeConstruct {
        CodeConstruct {
            kind: ConstructKind::Enum,
            name: name.into(),
            parameters: vec![],
            fields: vec![],
            variants: variants.iter().map(|s| s.to_string()).collect(),
            methods: vec![],
            line: 20,
        }
    }

    #[test]
    fn all_match() {
        let module = json!({
            "name": "StatusLed",
            "actions": [
                {"name": "Init", "params": [{"name": "pin", "type": "GpioPin", "direction": "in"}]},
            ],
            "state_machines": [
                {"name": "StateFsm", "states": ["Off", "On"]},
            ],
        });
        let code = vec![
            construct_struct("StatusLed"),
            construct_fn("init", vec![("pin", "GpioPin")]),
            construct_enum("StatusLedState", &["Off", "On"]),
        ];
        let items = compare_module(&module, &code, false, None);
        let matches = items.iter().filter(|i| matches!(i, AuditItem::Match { .. })).count();
        assert_eq!(matches, 3, "items: {items:?}");
    }

    #[test]
    fn missing_struct_and_action() {
        let module = json!({
            "name": "StatusLed",
            "actions": [
                {"name": "Init", "params": []},
            ],
        });
        let code: Vec<CodeConstruct> = vec![];
        let items = compare_module(&module, &code, false, None);
        let missing = items.iter().filter(|i| matches!(i, AuditItem::Missing { .. })).count();
        assert_eq!(missing, 2, "items: {items:?}");
    }

    #[test]
    fn mismatch_on_param_count() {
        let module = json!({
            "name": "StatusLed",
            "actions": [
                {"name": "Init", "params": [
                    {"name": "pin", "type": "GpioPin", "direction": "in"}
                ]},
            ],
        });
        let code = vec![
            construct_struct("StatusLed"),
            construct_fn("init", vec![]),
        ];
        let items = compare_module(&module, &code, false, None);
        let mismatches = items.iter().filter(|i| matches!(i, AuditItem::Mismatch { .. })).count();
        assert_eq!(mismatches, 1, "items: {items:?}");
    }

    #[test]
    fn uncovered_reports_extra_fn() {
        let module = json!({ "name": "StatusLed" });
        let code = vec![
            construct_struct("StatusLed"),
            construct_fn("debug_dump", vec![]),
        ];
        let items = compare_module(&module, &code, true, None);
        let uncovered = items.iter().filter(|i| matches!(i, AuditItem::Uncovered { .. })).count();
        assert_eq!(uncovered, 1, "items: {items:?}");
    }

    #[test]
    fn type_map_translates_and_matches() {
        let module = json!({
            "name": "StatusLed",
            "actions": [
                {"name": "SetLevel", "params": [
                    {"name": "level", "type": "Integer", "direction": "in"}
                ]},
            ],
        });
        let code = vec![
            construct_struct("StatusLed"),
            construct_fn("set_level", vec![("level", "i32")]),
        ];
        let type_map: HashMap<String, String> = [("Integer".into(), "i32".into())].into();
        let items = compare_module(&module, &code, false, Some(&type_map));
        let matches = items.iter().filter(|i| matches!(i, AuditItem::Match { kind, .. } if kind == "action")).count();
        assert_eq!(matches, 1, "items: {items:?}");
    }

    #[test]
    fn type_map_reference_strip() {
        let module = json!({
            "name": "StatusLed",
            "actions": [
                {"name": "Init", "params": [
                    {"name": "cfg", "type": "Config", "direction": "in"}
                ]},
            ],
        });
        let code = vec![
            construct_struct("StatusLed"),
            construct_fn("init", vec![("cfg", "&Config")]),
        ];
        let type_map: HashMap<String, String> = HashMap::new();
        let items = compare_module(&module, &code, false, Some(&type_map));
        let matches = items.iter().filter(|i| matches!(i, AuditItem::Match { kind, .. } if kind == "action")).count();
        assert_eq!(matches, 1, "items: {items:?}");
    }
}
