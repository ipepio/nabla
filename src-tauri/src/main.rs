// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if std::env::args().nth(1).as_deref() == Some(nabla_lib::MCP_STDIO_BRIDGE_FLAG) {
        std::process::exit(nabla_lib::run_mcp_stdio_bridge());
    }
    nabla_lib::run()
}
