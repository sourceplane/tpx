use std::{env, io, process};

fn main() {
    let cwd = match env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("failed to determine current directory: {error}");
            process::exit(1);
        }
    };
    let mut stdout = io::stdout();
    let mut stderr = io::stderr();
    let exit_code = match tpx_cli::run_cli_from(env::args_os(), &cwd, &mut stdout, &mut stderr) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("{error:#}");
            1
        }
    };

    process::exit(exit_code);
}
