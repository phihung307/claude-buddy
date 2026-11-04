//! Embeds the icon, manifest and file details, so Windows treats this as a proper app.

fn main() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=app.manifest");
    println!("cargo:rerun-if-changed=assets/app.ico");
    embed_resource::compile("app.rc", embed_resource::NONE).manifest_required().expect("embedding app.rc");
}
