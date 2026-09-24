//! `report imports`: a package a module's source imports but the module may
//! not rely on — the sibling-declared case that resolves today and breaks
//! when the sibling drops the dependency.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use specdev_analyzer::{
    bootstrap::{self, BootstrapInputs},
    imports,
    inline::run_inline_block,
    lang,
    layout::LayoutContext,
};

fn fixture_plugin() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/specs/full_plugin/_plugin")
}

fn fresh(tag: &str, profile: &str, language: &str) -> PathBuf {
    let target = std::env::temp_dir().join(format!(
        "specdev-report-imports-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let _ = fs::remove_dir_all(&target);
    fs::create_dir_all(&target).unwrap();
    bootstrap::run_bootstrap(
        &fixture_plugin(),
        &BootstrapInputs {
            profile: profile.into(),
            language: language.into(),
            project_name: "demo".into(),
            target_dir: target.clone(),
        },
    )
    .unwrap();
    target
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, text).unwrap();
}

fn run_cli(root: &Path) -> (i32, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_specdev-analyzer"))
        .env_remove("SPECDEV_PLUGIN_ROOT")
        .env_remove("CLAUDE_PLUGIN_ROOT")
        .args(["report", "imports", "--project"])
        .arg(root)
        .output()
        .unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn a_package_only_a_sibling_declares_is_reported() {
    let root = fresh("sibling", "nextjs-app", "typescript");
    write(
        &root,
        "specs/modules/Forms.toml",
        "name = \"Forms\"\nrole = \"service\"\n\n[[external_deps]]\npackage = \"zod\"\nversion = \"^4.0.0\"\n",
    );
    write(
        &root,
        "specs/modules/Orders.toml",
        "name = \"Orders\"\nrole = \"service\"\n",
    );
    write(
        &root,
        "src/forms.ts",
        "import { z } from \"zod\";\nexport const f = z;\n",
    );
    write(
        &root,
        "src/orders.ts",
        "import { z } from \"zod\";\nimport { f } from \"./forms\";\nimport x from \"@/lib/x\";\n\
         import fs from \"node:fs\";\nimport path from \"path\";\nimport \"react\";\n\
         export { y } from \"@scope/pkg/sub\";\nconst m = await import(\"lodash/fp\");\n",
    );

    let layout = LayoutContext::discover(&root, None).unwrap();
    let r = imports::run(&layout, None).unwrap();
    assert!(r.checked);
    assert_eq!(r.modules_checked, 2);
    let got: Vec<(String, String)> = r
        .undeclared
        .iter()
        .map(|u| (u.module.clone(), u.package.clone()))
        .collect();
    // `react` is an architecture-level dep of the profile; relative, alias,
    // `node:` and built-in specifiers name no package.
    assert_eq!(
        got,
        vec![
            ("Orders".to_string(), "@scope/pkg".to_string()),
            ("Orders".to_string(), "lodash".to_string()),
            ("Orders".to_string(), "zod".to_string()),
        ]
    );
    assert_eq!(r.undeclared[0].file, "src/orders.ts");

    let (code, stdout) = run_cli(&root);
    assert_eq!(code, 1, "{stdout}");
    assert!(stdout.contains("package: zod"), "{stdout}");

    // Declaring it on the module clears it.
    write(
        &root,
        "specs/modules/Orders.toml",
        "name = \"Orders\"\nrole = \"service\"\n\n[[external_deps]]\npackage = \"zod\"\nversion = \"^4.0.0\"\n\
         [[external_deps]]\npackage = \"lodash\"\nversion = \"^4.0.0\"\n\
         [[external_deps]]\npackage = \"@scope/pkg\"\nversion = \"^1.0.0\"\n",
    );
    assert_eq!(run_cli(&root).0, 0);
}

#[test]
fn a_test_only_dep_does_not_cover_a_runtime_import() {
    let root = fresh("testonly", "nextjs-app", "typescript");
    write(
        &root,
        "specs/modules/Orders.toml",
        "name = \"Orders\"\nrole = \"service\"\n\n[[external_deps]]\npackage = \"zod\"\nversion = \"^4.0.0\"\ntest_only = true\n",
    );
    write(&root, "src/orders.ts", "import { z } from \"zod\";\n");
    let layout = LayoutContext::discover(&root, None).unwrap();
    let r = imports::run(&layout, Some("Orders")).unwrap();
    assert_eq!(r.undeclared.len(), 1);
}

#[test]
fn deps_of_a_block_the_module_uses_count_and_of_one_it_does_not_do_not() {
    let root = fresh("block", "flutter-app", "dart");
    let layout = LayoutContext::discover(&root, None).unwrap();
    run_inline_block(&layout, &fixture_plugin(), "universal-ble").unwrap();
    let layout = LayoutContext::discover(&root, None).unwrap();
    let src = layout.spec.paths.source.clone();
    let body = "import 'package:universal_ble/universal_ble.dart';\n\
                import 'package:demo/src/other.dart';\n\
                import 'dart:async';\nimport 'helpers.dart';\n";
    // Uses the block: its `universal_ble` table makes the block relevant.
    write(
        &root,
        "specs/modules/Scanner.toml",
        "name = \"Scanner\"\nlayer = \"service\"\n\n[universal_ble]\n",
    );
    write(&root, &format!("{src}/scanner.dart"), body);
    // Does not use the block.
    write(
        &root,
        "specs/modules/Plain.toml",
        "name = \"Plain\"\nlayer = \"service\"\n",
    );
    write(&root, &format!("{src}/plain.dart"), body);

    let r = imports::run(&layout, None).unwrap();
    let got: Vec<(String, String)> = r
        .undeclared
        .iter()
        .map(|u| (u.module.clone(), u.package.clone()))
        .collect();
    // `package:demo` is the project itself; `dart:` and relative URIs name
    // no package.
    assert_eq!(
        got,
        vec![("Plain".to_string(), "universal_ble".to_string())]
    );
}

#[test]
fn a_language_without_an_import_query_is_not_checked() {
    let root = fresh("rust", "library-api", "rust");
    let (code, stdout) = run_cli(&root);
    assert_eq!(code, 0);
    assert!(stdout.contains("checked: false"), "{stdout}");
}

#[test]
fn adapters_map_specifiers_to_packages() {
    let ts = lang::adapter_for("typescript").unwrap();
    for (spec, want) in [
        ("zod", Some("zod")),
        ("zod/v4", Some("zod")),
        ("@a/b", Some("@a/b")),
        ("@a/b/c", Some("@a/b")),
        ("@a", None),
        ("./x", None),
        ("../x", None),
        ("/abs", None),
        ("@/lib", None),
        ("~/lib", None),
        ("#internal", None),
        ("node:fs", None),
        ("fs/promises", None),
        ("", None),
    ] {
        assert_eq!(ts.import_package(spec, "demo").as_deref(), want, "{spec}");
    }
    let dart = lang::adapter_for("dart").unwrap();
    for (spec, want) in [
        ("'package:http/http.dart'", Some("http")),
        ("\"package:http/http.dart\"", Some("http")),
        ("'package:demo/x.dart'", None),
        ("'dart:async'", None),
        ("'src/x.dart'", None),
    ] {
        assert_eq!(dart.import_package(spec, "demo").as_deref(), want, "{spec}");
    }
}
