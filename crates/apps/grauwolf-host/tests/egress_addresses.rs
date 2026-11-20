//! The default addresses of `grauwolf-egress` (the one table of the egress derivation) equal what
//! the providers really use.

use grauwolf_egress::providers::{
    BEDROCK_DEFAULT_REGION, PROVIDER_ADDRESSES, address_of, bedrock_url,
};
use grauwolf_host::providers::{BUILTIN, OPENAI_COMPATIBLE};

#[test]
fn the_table_has_one_row_per_built_in_provider() {
    let mut table: Vec<&str> = PROVIDER_ADDRESSES.iter().map(|row| row.provider).collect();
    let mut builtin = BUILTIN.to_vec();
    table.sort_unstable();
    builtin.sort_unstable();
    assert_eq!(table, builtin);
}

#[test]
fn the_compatible_presets_have_the_address_and_variable_of_the_providers() {
    for compatible in OPENAI_COMPATIBLE {
        let row = address_of(compatible.provider).unwrap();
        assert_eq!(row.default_url, compatible.default_base_url);
        assert_eq!(row.from_env, Some(compatible.base_url));
    }
}

#[test]
fn the_adapters_default_addresses_are_the_table() {
    let default = |provider: &str| address_of(provider).unwrap().default_url;
    assert_eq!(default("ollama"), grauwolf_ollama::DEFAULT_BASE_URL);
    assert_eq!(default("openai"), grauwolf_openai::DEFAULT_BASE_URL);
    assert_eq!(default("anthropic"), grauwolf_anthropic::DEFAULT_BASE_URL);
    assert_eq!(default("gemini"), grauwolf_gemini::DEFAULT_BASE_URL);
    assert_eq!(BEDROCK_DEFAULT_REGION, grauwolf_bedrock::DEFAULT_REGION);
    assert_eq!(
        bedrock_url("eu-west-1"),
        grauwolf_bedrock::default_base_url("eu-west-1")
    );
}
