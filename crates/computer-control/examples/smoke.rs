// 真机冒烟：cargo run --example smoke -- capture <out.jpg> | move X Y | click X Y | text STR | key CHORD | own X Y
use base64::Engine;
use computer_control::{
    capture_screen, check_support, foreground_is_own, parse_chord, point_hits_own_window, Button,
    Controller,
};

fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let number = |index: usize| {
        args[index]
            .parse::<i32>()
            .map_err(|error| error.to_string())
    };
    match args.first().map(String::as_str) {
        Some("support") => println!("{:?}", check_support()),
        Some("capture") => {
            let shot = capture_screen(1440, 80)?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(shot.jpeg_base64)
                .map_err(|error| error.to_string())?;
            std::fs::write(&args[1], bytes).map_err(|error| error.to_string())?;
            println!("{:?}", shot.map);
        }
        Some("move") => Controller::new()?.move_to(number(1)?, number(2)?)?,
        Some("click") => {
            let mut controller = Controller::new()?;
            controller.move_to(number(1)?, number(2)?)?;
            controller.click(Button::Left)?;
        }
        Some("scroll") => Controller::new()?.scroll(number(1)?, false)?,
        Some("text") => Controller::new()?.text(&args[1])?,
        Some("key") => Controller::new()?.chord(&parse_chord(&args[1])?)?,
        Some("own") => println!(
            "point={} foreground={}",
            point_hits_own_window(number(1)?, number(2)?),
            foreground_is_own()
        ),
        _ => return Err("unknown command".into()),
    }
    Ok(())
}
