fn main() {
    // Манифест требует запуска от администратора — без этого нельзя читать MFT.
    // embed-resource компилирует .rc-скрипт, поэтому манифест подключается через app.manifest.rc.
    embed_resource::compile("app.manifest.rc", embed_resource::NONE)
        .manifest_required()
        .unwrap();
    tauri_build::build()
}
