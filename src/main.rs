use clap::{Parser, Subcommand};
use fs_err::PathExt;
use read_input::prelude::*;
use std::ffi::OsString;
use std::path::Path;
use wr::{ExerciseCollection, ExerciseDefinition, ExercisesConfig, OpenedExercise, Verification};
use yansi::Paint;

/// A small CLI to manage test-driven workshops and tutorials in Rust.
///
/// Each exercise comes with a set of associated tests.
/// A suite of exercises is called "collection".
///
/// Invoking `wr` runs tests for all the exercises you have opened so far in a collection
/// to check if your solutions are correct.
/// If everything runs smoothly, you will be asked if you want to move forward to the next exercise.
#[derive(Parser)]
#[command(author, version, about, long_about = None)]
pub struct Command {
    #[arg(long)]
    /// Compile and run tests for all opened exercises, even if they have already succeeded
    /// in a past run.
    pub recheck: bool,

    #[arg(long)]
    /// By default, `wr` will run `cargo build` in quiet mode and it won't show you the logs
    /// coming from the build process.
    /// With this flag, those logs (and the progress bar) will be displayed.
    pub verbose: bool,

    #[arg(long)]
    /// By default, `wr` will prompt you to open the next exercise if all the currently opened
    /// exercises passed their tests.
    /// With this flag, `wr` will automatically open the next exercise if all the currently opened
    /// exercises passed their tests. It'll then run the tests for the newly opened exercise.
    /// If they pass, it'll open the next one, and so on.
    pub keep_going: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Open a specific exercise.
    ///
    /// You can either provide the full name of the chapter and exercise, or only their number.
    ///
    /// E.g. `wr open --chapter 01_structured_logging --exercise 00_intro` will open
    /// the exercise located at `01_structured_logging/00_intro`.
    /// The same exercise can be opened with `wr open --chapter 1 --exercise 0`.
    Open {
        /// The name of the chapter containing the exercise, or its number.
        ///
        /// E.g. `--chapter 01_structured_logging` and `--chapter 1` are equivalent.
        #[arg(long)]
        chapter: String,
        /// The name of the exercise, or its number within the chapter it belongs to.
        ///
        /// E.g. `--exercise 00_intro` and `--exercise 0` are equivalent.
        #[arg(long)]
        exercise: String,
    },
    /// Run the tests for the exercise in the current directory.
    /// It errors if the current directory is not an exercise.
    Check,
}

fn main() -> Result<(), anyhow::Error> {
    let command = Command::parse();
    // Enable ANSI colour support on Windows, if it's supported.
    // Disable it entirely otherwise.
    if !use_ansi_colours() {
        Paint::disable();
    }
    let configuration = ExercisesConfig::load()?;
    let verbose = command.verbose;
    let mut exercises = ExerciseCollection::new(configuration.exercises_dir().to_path_buf())?;

    if let Some(command) = command.command {
        match command {
            Commands::Open { chapter, exercise } => {
                enum Selector {
                    FullName(String),
                    Number(u16),
                }

                impl Selector {
                    fn new(s: String) -> Self {
                        match s.parse::<u16>() {
                            Ok(number) => Selector::Number(number),
                            Err(_) => Selector::FullName(s),
                        }
                    }

                    fn matches(&self, name: &str, number: u16) -> bool {
                        match self {
                            Selector::FullName(s) => s == name,
                            Selector::Number(n) => *n == number,
                        }
                    }
                }

                impl std::fmt::Display for Selector {
                    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                        match self {
                            Selector::FullName(s) => write!(f, "{}", s),
                            Selector::Number(n) => write!(f, "{}", n),
                        }
                    }
                }

                let chapter_selector = Selector::new(chapter);
                let exercise_selector = Selector::new(exercise);

                let exercise = exercises.iter().find(|k| {
                    chapter_selector.matches(&k.chapter(), k.chapter_number())
                        && exercise_selector.matches(&k.exercise(), k.exercise_number())
                }).ok_or_else(|| {
                    anyhow::anyhow!("There is no exercise matching `--chapter {chapter_selector} -- exercise {exercise_selector}`")
                })?.to_owned();

                exercises.open(&exercise)?;
                print_opened_message(&exercise, exercises.exercises_dir());
            }
            Commands::Check => {
                let current_dir = std::env::current_dir()?.fs_err_canonicalize()?;
                let definition = exercises
                    .iter()
                    .find(|k| {
                        let manifest_folder = k
                            .manifest_folder_path(exercises.exercises_dir())
                            .fs_err_canonicalize()
                            .expect("Failed to canonicalize manifest folder path");
                        manifest_folder == current_dir
                    })
                    .ok_or_else(|| anyhow::anyhow!("The current directory is not an exercise"))?;
                verify(
                    &exercises,
                    &definition,
                    configuration.verification(),
                    configuration.skip_build,
                    verbose,
                )?;
            }
        }
        return Ok(());
    }

    // If no command was specified, we verify the user's progress on the workshop-runner that have already
    // been opened.
    if let TestOutcome::Failure { command, details } = seek_the_path(
        &mut exercises,
        command.recheck,
        configuration.verification(),
        configuration.skip_build,
        verbose,
    )? {
        print_failure_message(&command, &details);
        std::process::exit(1);
    };

    // If all the currently opened workshop-runner passed their checks, we open the next one (if it exists).
    while let Some(next_exercise) = exercises.next()? {
        if command.keep_going {
            let next_exercise = exercises
                .open_next()
                .expect("Failed to open the next exercise");
            let exercise_outcome = verify(
                &exercises,
                &next_exercise,
                configuration.verification(),
                configuration.skip_build,
                command.verbose,
            )?;
            if let TestOutcome::Failure { command, details } = exercise_outcome {
                print_failure_message(&command, &details);
                std::process::exit(1);
            };
            continue;
        } else {
            println!(
                "\t{}\n",
                info_style().paint(
                    "Eternity lies ahead of us, and behind. Your path is not yet finished. 🍂"
                )
            );

            let open_next = input::<String>()
                .repeat_msg(format!(
                    "Do you want to open the next exercise, {}? [y/n] ",
                    next_exercise
                ))
                .err("Please answer either yes or no.")
                .add_test(|s| parse_bool(s).is_some())
                .get();
            // We can safely unwrap here because we have already validated the input.
            let open_next = parse_bool(&open_next).unwrap();

            if open_next {
                let next_exercise = exercises
                    .open_next()
                    .expect("Failed to open the next exercise");
                print_opened_message(&next_exercise, exercises.exercises_dir());
            }
            return Ok(());
        }
    }
    println!(
        "{}\n\t{}\n",
        success_style().paint("\n\tThere will be no more tasks."),
        info_style().paint("What is the sound of one hand clapping (for you)? 🌟")
    );
    Ok(())
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "yes" | "y" => Some(true),
        "no" | "n" => Some(false),
        _ => None,
    }
}

fn seek_the_path(
    exercises: &mut ExerciseCollection,
    recheck: bool,
    verification: &[Verification],
    skip_build: bool,
    verbose: bool,
) -> Result<TestOutcome, anyhow::Error> {
    println!(" \n\n{}", info_style().dimmed().paint("Running tests...\n"));
    for exercise in exercises.opened()? {
        let OpenedExercise { definition, solved } = &exercise;
        if !exercise.definition.exists(exercises.exercises_dir()) {
            exercises.close(&definition)?;
            continue;
        }
        if *solved && !recheck {
            println!(
                "{}",
                info_style().paint(format!("\t⏩ {} (Not rechecked)", definition))
            );
            continue;
        }
        let exercise_outcome = verify(exercises, &definition, verification, skip_build, verbose)?;
        if let TestOutcome::Failure { command, details } = exercise_outcome {
            return Ok(TestOutcome::Failure { command, details });
        }
    }
    Ok(TestOutcome::Success)
}

fn verify(
    exercises: &ExerciseCollection,
    definition: &ExerciseDefinition,
    verification: &[Verification],
    skip_build: bool,
    verbose: bool,
) -> Result<TestOutcome, anyhow::Error> {
    let exercise_config = definition.config(exercises.exercises_dir())?;
    // Exercise-specific config takes precedence over the global one, if specified.
    let verification = exercise_config
        .as_ref()
        .map(|c| c.verification.as_slice())
        .unwrap_or(verification);
    let exercise_outcome = _verify(
        &definition.manifest_path(exercises.exercises_dir()),
        verification,
        skip_build,
        verbose,
    );
    match &exercise_outcome {
        TestOutcome::Success => {
            println!("{}", success_style().paint(format!("\t🚀 {}", definition)));
            exercises.mark_as_solved(&definition)?;
        }
        TestOutcome::Failure { .. } => {
            println!("{}", failure_style().paint(format!("\t❌ {}", definition)));
            exercises.mark_as_unsolved(&definition)?;
        }
    }
    Ok(exercise_outcome)
}

fn _verify(
    manifest_path: &Path,
    verification: &[Verification],
    skip_build: bool,
    verbose: bool,
) -> TestOutcome {
    // Tell cargo to return colored output, unless we are on Windows and the terminal
    // doesn't support it.
    let color_option = if use_ansi_colours() {
        "always"
    } else {
        "never"
    };

    // `cargo build` first
    if !skip_build {
        let mut cmd = std::process::Command::new("cargo");
        cmd.arg("build");
        cmd.arg("--manifest-path");
        cmd.arg(manifest_path);
        cmd.arg("--all-targets");
        cmd.arg("--color");
        cmd.arg(color_option);
        if !verbose {
            cmd.arg("-q");
        }

        if verbose {
            cmd.stdout(std::process::Stdio::inherit())
                .stderr(std::process::Stdio::inherit());
        }

        let output = cmd.output().expect("Failed to build the project");

        if !output.status.success() {
            return TestOutcome::Failure {
                command: format!("{:?}", cmd),
                details: [output.stderr, output.stdout].concat(),
            };
        }
    }

    // Now we run the verification command.
    {
        let mut verification_commands: Vec<_> = verification
            .iter()
            .map(|v| {
                let mut cmd = std::process::Command::new(&v.command);
                cmd.args(&v.args);
                cmd
            })
            .collect();
        if verification_commands.is_empty() {
            let mut args: Vec<OsString> =
                vec!["test".into(), "--color".into(), color_option.into()];

            if !verbose {
                args.push("-q".into());
            }

            let mut cmd = std::process::Command::new("cargo");
            cmd.args(args);
            verification_commands.push(cmd);
        }
        verification_commands.iter_mut().for_each(|cmd| {
            // We run verification commands from the exercise's directory.
            cmd.current_dir(
                manifest_path
                    .parent()
                    .expect("Failed to get parent dir for manifest"),
            );
        });
        for mut verification_cmd in verification_commands {
            let error_msg = format!("Failed to run: `{:?}`", verification_cmd);
            let output = verification_cmd.output().expect(&error_msg);

            if !output.status.success() {
                return TestOutcome::Failure {
                    command: format!("{:?}", verification_cmd),
                    details: [output.stderr, output.stdout].concat(),
                };
            }
        }
    }

    TestOutcome::Success
}

#[derive(PartialEq)]
enum TestOutcome {
    Success,
    Failure { command: String, details: Vec<u8> },
}

fn print_opened_message(exercise: &ExerciseDefinition, exercises_dir: &Path) {
    println!(
        "{} {}",
        next_style().paint("\n\tAhead of you lies"),
        next_style().bold().paint(format!("{exercise}")),
    );
    let relative_path = exercise.manifest_folder_path(exercises_dir);
    let open_msg = format!(
        "\n\tOpen {:?} in your editor and get started!\n\tRun `wr` again to compile the exercise and execute its tests.",
        relative_path
    );
    println!("{}", next_style().paint(open_msg));
}

fn print_failure_message(command: &str, details: &[u8]) {
    println!(
        "\n\t{}\n\nFailed to run:\n\t{}\nOutput:\n{}\n",
        info_style()
            .paint("Meditate on your approach and return. Mountains are merely mountains.\n\n"),
        cargo_style().paint(&command),
        cargo_style().paint(textwrap::indent(
            &String::from_utf8_lossy(details).to_string(),
            "\t"
        ))
    );
}

pub fn info_style() -> yansi::Style {
    yansi::Style::new(yansi::Color::Default)
}
pub fn cargo_style() -> yansi::Style {
    yansi::Style::new(yansi::Color::Default).dimmed()
}
pub fn next_style() -> yansi::Style {
    yansi::Style::new(yansi::Color::Yellow)
}
pub fn success_style() -> yansi::Style {
    yansi::Style::new(yansi::Color::Green)
}
pub fn failure_style() -> yansi::Style {
    yansi::Style::new(yansi::Color::Red)
}

/// Determine if our terminal output should leverage colors via ANSI escape codes.
pub fn use_ansi_colours() -> bool {
    if cfg!(target_os = "windows") {
        Paint::enable_windows_ascii()
    } else {
        true
    }
}
