//! 生成插件桌面脚本的 TypeScript 声明（gpui-shell 组件目录 + `fluxdown` 模块）。
//!
//! 用法：
//! - `gen_plugin_typings <out>`：写入 `<out>`；
//! - `gen_plugin_typings --check <path>`：与 `<path>` 比较，不一致退出码 1。

use std::{path::PathBuf, process::ExitCode};

use gpui_shell::policy::{Policy, set_default};

fn generate() -> Result<String, String> {
    let components = gpui_shell::components::components().map_err(|error| error.to_string())?;
    let policy = Policy::new()
        .with_host_module(fluxdown_ui_plugin_ui::fluxdown_module_declarations_only())
        .map_err(|error| error.message().to_owned())?;
    set_default(policy);
    Ok(gpui_shell::type_declarations(&components))
}

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let (check, path) = match arguments.as_slice() {
        [flag, path] if flag == "--check" => (true, PathBuf::from(path)),
        [path] if path != "--check" => (false, PathBuf::from(path)),
        _ => {
            eprintln!("usage: gen_plugin_typings <out> | --check <path>");
            return ExitCode::from(2);
        }
    };
    let generated = match generate() {
        Ok(text) => text,
        Err(error) => {
            eprintln!("failed to generate declarations: {error}");
            return ExitCode::from(2);
        }
    };
    if check {
        match std::fs::read_to_string(&path) {
            Ok(current) if current == generated => ExitCode::SUCCESS,
            Ok(_) => {
                eprintln!(
                    "{} is out of date; regenerate with `cargo run -p fluxdown_ui_plugin_ui --example gen_plugin_typings -- {}`",
                    path.display(),
                    path.display()
                );
                ExitCode::from(1)
            }
            Err(error) => {
                eprintln!("cannot read {}: {error}", path.display());
                ExitCode::from(1)
            }
        }
    } else {
        if let Some(parent) = path.parent()
            && let Err(error) = std::fs::create_dir_all(parent)
        {
            eprintln!("cannot create {}: {error}", parent.display());
            return ExitCode::from(2);
        }
        match std::fs::write(&path, generated) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("cannot write {}: {error}", path.display());
                ExitCode::from(2)
            }
        }
    }
}
