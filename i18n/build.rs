use std::{collections::BTreeMap, env, fs, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=../../i18n/build.rs");
    println!("cargo:rerun-if-changed=locales");
    println!("cargo:rerun-if-env-changed=VC_LOCALE");
    let locale = env::var("VC_LOCALE").unwrap_or_else(|_| "ja".into());
    assert!(
        locale
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "invalid VC_LOCALE"
    );
    println!("cargo:rustc-env=VC_UI_LOCALE={locale}");
    let mut messages: BTreeMap<String, String> =
        serde_json::from_str(&fs::read_to_string("locales/ja.json").unwrap()).unwrap();
    if locale != "ja" {
        let translations: BTreeMap<String, String> = serde_json::from_str(
            &fs::read_to_string(format!("locales/{locale}.json"))
                .expect("locale catalog not found"),
        )
        .expect("invalid locale catalog");
        for (key, value) in translations {
            assert!(
                messages.contains_key(&key),
                "unknown translation key: {key}"
            );
            messages.insert(key, value);
        }
    }
    // Literal expansion preserves const documentation and Rust's format checking.
    let mut source = String::from("macro_rules! message {\n");
    for (key, value) in messages {
        source.push_str(&format!("({key:?}) => {{ {value:?} }};\n"));
    }
    source.push_str("}\n");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("messages.rs"),
        source,
    )
    .unwrap();
}
