fn main() {
    // Манифест требует запуска от администратора — без этого нельзя читать MFT.
    // embed-resource компилирует .rc-скрипт, поэтому манифест подключается через app.manifest.rc.
    // compile_for — только к бинарю приложения, иначе манифест попадёт и в тестовые
    // бинарники, и `cargo test` будет требовать запуска от администратора.
    embed_resource::compile_for("app.manifest.rc", &["woozytree"], embed_resource::NONE)
        .manifest_required()
        .unwrap();
    // Свой манифест уже встроен выше, поэтому стандартный манифест Tauri не нужен
    // (иначе линковщик ругается на дублирующийся ресурс MANIFEST).
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .windows_attributes(tauri_build::WindowsAttributes::new_without_app_manifest()),
    )
    .expect("failed to run tauri-build");
}
