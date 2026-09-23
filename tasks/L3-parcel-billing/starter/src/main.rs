use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parcelflow::cli::parse_args(&args).and_then(parcelflow::cli::execute) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("parcelflow: {e}");
            if matches!(e, parcelflow::Error::Usage(_)) {
                eprintln!("{}", parcelflow::cli::USAGE);
            }
            ExitCode::FAILURE
        }
    }
}
