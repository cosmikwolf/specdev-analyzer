//! Phase 44: the flxs1 session's second round of field feedback (#226–#231).
//!
//! Each test was written red first, against the 0.3.1 behaviour.

use crate::ci;

use ci::{fw_module, tree};

const RTIC_APP: &str = r#"#![no_std]
#![no_main]
#[rtic::app(device = mk20dx_hal, dispatchers = [CAN0_ORed_Message_buffer])]
mod app {
    #[shared] struct Shared {}
    #[local] struct Local {}
    #[init] fn init(_cx: init::Context) -> (Shared, Local) { (Shared {}, Local {}) }
    #[task(priority = 1)] async fn built(_cx: built::Context) {}
}
"#;

fn task(name: &str, extra: &str) -> String {
    format!("[[rtic.tasks]]\nname = \"{name}\"\nkind = \"software\"\npriority = 1\n{extra}\n")
}

/// #231: the entry-level `planned` exit was not discoverable. The finding for a
/// missing pack entry names it; a planned module whose source exists and which
/// has pack entries is told it can flag the entries instead; a planned module
/// with no pack entries is not.
#[test]
fn the_planned_exits_are_named_where_they_apply() {
    let t = tree("planned-exits", "firmware-layered", &["rtic", "mk20dx"]);
    let main = "source_file = \"src/main.rs\"\n";
    t.write("src/main.rs", RTIC_APP)
        .write("src/plain.rs", "pub fn plain() {}\n")
        .write(
            "specs/modules/App.toml",
            &(fw_module("App", main) + &task("built", "") + &task("not_yet", "")),
        )
        .write(
            "specs/modules/Target.toml",
            &(fw_module("Target", &format!("planned = true\n{main}")) + &task("built", "")),
        )
        .write(
            "specs/modules/Plain.toml",
            &fw_module("Plain", "planned = true\n"),
        );
    let c = t.check(&[]);
    c.fires("pack_entity_in_code", "App.not_yet", "error")
        .says(
            "pack_entity_in_code",
            "App.not_yet",
            "set `planned = true` on this entry",
        )
        .fires("stale_planned", "Target", "warning")
        .says(
            "stale_planned",
            "Target",
            "`planned = true` on those entries instead",
        )
        .fires("stale_planned", "Plain", "warning");
    let plain = c
        .ran()
        .iter()
        .find(|f| f.rule == "stale_planned" && f.entity == "Plain")
        .unwrap();
    assert!(!plain.message.contains("entries"), "{}", plain.message);
}

/// #228: a module whose code lives at another path. The finding ranks the
/// files that define the module's names (at any visibility: task functions
/// are `pub(crate)`), and stays as it was when no file defines any.
#[test]
fn source_resolves_names_the_files_that_define_the_module() {
    let t = tree("source-hint", "firmware-layered", &[]);
    t.write(
        "specs/modules/FlashTask.toml",
        &(fw_module("FlashTask", "")
            + "[[actions]]\nname = \"flash_task\"\n\n[[actions]]\nname = \"init_buffer\"\n"),
    )
    .write(
        "specs/modules/Ghost.toml",
        &(fw_module("Ghost", "") + "[[actions]]\nname = \"never_written\"\n"),
    )
    .write(
        "src/tasks/flash.rs",
        "pub(crate) async fn flash_task() {}\nfn init_buffer() {}\n",
    )
    .write(
        "src/tasks/mod.rs",
        "pub(crate) mod flash;\npub fn init_buffer() {}\n",
    );
    let c = t.check(&[]);
    c.fires("source_resolves", "FlashTask", "error")
        .says(
            "source_resolves",
            "FlashTask",
            "candidates: `src/tasks/flash.rs` defines 2 of 2 name(s) (`flash_task`, `init_buffer`); \
             `src/tasks/mod.rs` defines 1 of 2 name(s) (`init_buffer`)",
        )
        .fires("source_resolves", "Ghost", "error");
    let ghost = c
        .ran()
        .iter()
        .find(|f| f.rule == "source_resolves" && f.entity == "Ghost")
        .unwrap();
    assert!(!ghost.message.contains("candidates"), "{}", ghost.message);
}
