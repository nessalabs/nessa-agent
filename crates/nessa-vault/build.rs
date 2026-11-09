use std::{env, fs, path::PathBuf};
fn main() {
    let path = "../../protocol/defaults/vault-contract.json";
    println!("cargo:rerun-if-changed={path}");
    let values: serde_json::Map<String, serde_json::Value> =
        serde_json::from_slice(&fs::read(path).expect("vault contract readable"))
            .expect("vault contract object");
    let mut code = String::new();
    for (name, value) in values {
        let mut constant = String::new();
        for character in name.chars() {
            if character.is_uppercase() {
                constant.push('_');
            }
            constant.extend(character.to_uppercase());
        }
        let value = value
            .as_u64()
            .filter(|value| *value > 0 && *value <= i32::MAX as u64)
            .expect("positive bounded vault contract");
        code.push_str(&format!("pub const {constant}: usize = {value};\n"));
    }
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR")).join("vault_contract.rs"),
        code,
    )
    .expect("write vault contract");
}
