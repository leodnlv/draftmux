fn main() {
    // Embeds the app icon in draftmux.exe; does nothing for other targets.
    let resources = "../../windows";
    println!("cargo:rerun-if-changed={resources}");
    embed_resource::compile_for(
        format!("{resources}/draftmux.rc"),
        ["draftmux"],
        embed_resource::ParamsIncludeDirs([resources]),
    )
    .manifest_required()
    .unwrap();
}
