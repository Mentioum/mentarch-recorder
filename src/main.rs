mod engine;
mod model;
mod output;
mod portal;
mod status;
mod ui;
mod worker;

use anyhow::Result;

fn main() {
    if let Err(error) = run() {
        eprintln!("mentarch-recorder: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--check") => {
            gstreamer::init()?;
            engine::check_plugins()?;
            worker::runtime()?.block_on(portal::check())?;
            let encoder = engine::select_encoder()?;
            println!(
                "Ready. Encoder: {}. Output: {}",
                encoder.label,
                output::recordings_dir()?.display()
            );
        }
        Some("--self-test") => {
            gstreamer::init()?;
            let dir = args
                .get(1)
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::env::temp_dir().join("mentarch-recorder-test"));
            let path = engine::self_test(&dir)?;
            println!("Saved test recording: {}", path.display());
        }
        Some("--capture-test") => {
            gstreamer::init()?;
            worker::runtime()?.block_on(worker::capture_test())?;
        }
        Some("--waybar") => println!("{}", status::waybar(&status::read())),
        Some("--status") => println!("{}", serde_json::to_string(&status::read())?),
        Some("--help" | "-h") => println!(
            "mentarch-recorder\n\n  No arguments / --show   Show recorder controls\n  --toggle               Choose & record, or stop & save\n  --start                Choose a window and record automatically\n  --choose               Choose a window without recording yet\n  --record               Record the selected window\n  --stop                 Stop & save\n  --cancel               Cancel selection (native dialog must be dismissed)\n  --open / --folder      Open latest video / recordings folder\n  --quit                 Save, close capture, and quit\n  --background           Run controls without showing a window\n  --waybar               Print Waybar JSON status\n  --status               Print full JSON status\n  --check                Check portal and encoder support\n  --self-test [DIR]       Encode a synthetic test video\n  --capture-test          Pick a window and record for ten seconds"
        ),
        Some(other) => match model::Action::parse(other) {
            Some(action) => ui::run(action),
            None => anyhow::bail!("Unknown argument {other}. Use --help."),
        },
        None => ui::run(model::Action::Show),
    }
    Ok(())
}
