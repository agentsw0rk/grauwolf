//! The registry of a harness's default image (ADR-0128 H7): `[harness] registry` of the nearest
//! `grauwolf.toml` above the harness file.

use grauwolf_test::TestDir;

#[test]
fn the_registry_comes_from_the_nearest_grauwolf_toml_above_the_harness_file() {
    let dir = TestDir::new("harness-registry");
    std::fs::create_dir_all(dir.join("harnesses")).unwrap();
    std::fs::write(
        dir.join("grauwolf.toml"),
        "[credentials]\n\n[harness]\nregistry = \"ghcr.io/agentsw0rk\"\n",
    )
    .unwrap();
    let harness = dir.join("harnesses/bot.harness.json");

    assert_eq!(
        grauwolf_agent_tree::harness_registry(&harness),
        Ok(Some("ghcr.io/agentsw0rk".to_owned()))
    );
}
