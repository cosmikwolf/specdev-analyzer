//! The shipped rtic rule pack against code shapes it must accept: a task
//! whose `#[task]` attribute is not directly above the `fn` (a second
//! attribute, a doc comment between them) is still found and checked.

#[path = "common/checked_intent.rs"]
mod ci;

use ci::{fw_module, tree};

const APP: &str = r#"#![no_std]
#![no_main]
#[rtic::app(device = mk20dx_hal, dispatchers = [CAN0_ORed_Message_buffer])]
mod app {
    #[shared] struct Shared {}
    #[local] struct Local {}
    #[init] fn init(_cx: init::Context) -> (Shared, Local) { (Shared {}, Local {}) }
    #[task(binds = PIT0, priority = 3)]
    #[allow(unused)]
    fn two_attrs(_cx: two_attrs::Context) {}
    #[task(priority = 2)]
    /// A doc comment between the attribute and the function.
    async fn doc_between(_cx: doc_between::Context) {}
    /// A doc comment above both.
    #[task(priority = 1)]
    async fn doc_above(_cx: doc_above::Context) {}
    #[idle]
    #[allow(unused)]
    fn idle(_cx: idle::Context) -> ! { loop {} }
}
"#;

#[test]
fn tasks_are_found_through_other_attributes_and_comments() {
    let t = tree("rtic-shapes", "firmware-layered", &["rtic", "mk20dx"]);
    let tasks = "[[rtic.tasks]]\nname = \"two_attrs\"\nkind = \"hardware\"\nbinds = \"PIT0\"\npriority = 3\n\n\
                 [[rtic.tasks]]\nname = \"doc_between\"\nkind = \"software\"\npriority = 1\n\n\
                 [[rtic.tasks]]\nname = \"doc_above\"\nkind = \"software\"\npriority = 1\n\n\
                 [[rtic.tasks]]\nname = \"idle\"\nkind = \"idle\"\n";
    t.write("src/main.rs", APP)
        .write("specs/modules/App.toml", &(fw_module("App", "source_file = \"src/main.rs\"\n") + tasks));
    let c = t.check(&[]);
    for task in ["App.two_attrs", "App.doc_between", "App.doc_above", "App.idle"] {
        c.silent("pack_entity_in_code", task);
    }
    c.silent("pack_attribute_matches", "App.two_attrs")
        .silent("pack_attribute_matches", "App.doc_above")
        .silent("pack_attribute_matches", "App.idle")
        // Spec priority 1, code 2: the arguments are read through the comment.
        .fires("pack_attribute_matches", "App.doc_between", "error");
}

const RESOURCES_APP: &str = r#"#![no_std]
#![no_main]
#[rtic::app(device = mk20dx_hal, dispatchers = [CAN0_ORed_Message_buffer])]
mod app {
    #[shared]
    struct Shared { count: u32, wrong_place: u8 }
    /// Doc comment between the attribute and the struct.
    #[local]
    struct Local { led_on: bool, chip_handle: u8 }
    #[init] fn init(_cx: init::Context) -> (Shared, Local) { loop {} }
    #[task(local = [buf: [u8; 4] = [0; 4], led_on], priority = 1)]
    async fn work(_cx: work::Context) {}
}
"#;

#[test]
fn resources_are_matched_by_struct_or_inline_local_and_sharing() {
    let t = tree("rtic-resources", "firmware-layered", &["rtic", "mk20dx"]);
    let res = |n: &str, sharing: &str| format!("[[rtic.resources]]\nname = \"{n}\"\nsharing = \"{sharing}\"\ntype = \"u8\"\n\n");
    let body = format!(
        "[[rtic.tasks]]\nname = \"work\"\nkind = \"software\"\npriority = 1\n\n{}{}{}{}{}",
        res("count", "shared"),
        res("led_on", "local"),
        res("buf", "local"),
        // In the code's `Shared` struct, but the spec says local.
        res("wrong_place", "local"),
        res("gone", "shared"),
    );
    t.write("src/main.rs", RESOURCES_APP)
        .write("specs/modules/App.toml", &(fw_module("App", "source_file = \"src/main.rs\"\n") + &body));
    let c = t.check(&[]);
    for r in ["App.count", "App.led_on", "App.buf"] {
        c.silent("pack_entity_in_code", r).silent("pack_attribute_matches", r);
    }
    c.fires("pack_attribute_matches", "App.wrong_place", "error")
        .fires("pack_entity_in_code", "App.gone", "error")
        // A code field the spec does not name is not reported.
        .nothing_about("App.chip_handle");
}
