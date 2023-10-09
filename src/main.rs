use clap::{Parser, Subcommand};
use read_input::prelude::*;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use wr::{ExerciseCollection, ExerciseDefinition, OpenedExercise};
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
    /// Path to the exercise collection you want to work on.
    /// Both absolute and relative paths are supported.
    ///
    /// E.g. `wr --path my_exercises` if `my_exercise` is a sub-directory of your current
    /// working directory.
    ///
    /// It defaults to `exercises` if not specified.
    #[arg(long, default_value = "exercises")]
    pub path: PathBuf,

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
}

fn main() -> Result<(), anyhow::Error> {
    let command = Command::parse();
    // Enable ANSI colour support on Windows, if it's supported.
    // Disable it entirely otherwise.
    if !use_ansi_colours() {
        Paint::disable();
    }
    let mut exercises = ExerciseCollection::new(command.path)?;

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
        }
        return Ok(());
    }

    // If no command was specified, we verify the user's progress on the workshop-runner that have already
    // been opened.
    if let TestOutcome::Failure { details } = seek_the_path(&exercises)? {
        println!(
            "\n\t{}\n\n{}\n\n",
            info_style()
                .paint("Meditate on your approach and return. Mountains are merely mountains.\n\n"),
            cargo_style().paint(&String::from_utf8_lossy(&details).to_string())
        );
        // FIXME: return a non-zero exit code.
        return Ok(());
    };

    // If all the currently opened workshop-runner passed their checks, we open the next one (if it exists).
    match exercises.next()? {
        Some(next_exercise) => {
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
        }
        None => {
            println!(
                "{}\n\t{}\n",
                success_style().paint("\n\tThere will be no more tasks."),
                info_style().paint("What is the sound of one hand clapping (for you)? 🌟")
            );
        }
    }
    Ok(())
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "yes" | "y" => Some(true),
        "no" | "n" => Some(false),
        _ => None,
    }
}

fn seek_the_path(exercises: &ExerciseCollection) -> Result<TestOutcome, anyhow::Error> {
    println!(" \n\n{}", info_style().dimmed().paint("Running tests...\n"));
    for exercise in exercises.opened()? {
        let OpenedExercise { definition, solved } = &exercise;
        if *solved {
            println!(
                "{}",
                info_style().paint(format!("\t✅ {} (Skipped)", definition))
            );
            continue;
        }
        let exercise_outcome = run_tests(&definition.manifest_path(exercises.exercises_dir()));
        match exercise_outcome {
            TestOutcome::Success => {
                println!("{}", success_style().paint(format!("\t🚀 {}", definition)));
                exercises.mark_as_solved(&exercise)?;
            }
            TestOutcome::Failure { details } => {
                println!("{}", failure_style().paint(format!("\t❌ {}", definition)));
                exercises.mark_as_unsolved(&exercise)?;
                return Ok(TestOutcome::Failure { details });
            }
        }
    }
    Ok(TestOutcome::Success)
}

fn run_tests(manifest_path: &Path) -> TestOutcome {
    // Tell cargo to return colored output, unless we are on Windows and the terminal
    // doesn't support it.
    let color_option = if use_ansi_colours() {
        "always"
    } else {
        "never"
    };

    // `cargo build` first
    {
        let args: Vec<OsString> = vec![
            "build".into(),
            "--manifest-path".into(),
            manifest_path.into(),
            "-q".into(),
            "--color".into(),
            color_option.into(),
        ];

        let output = std::process::Command::new("cargo")
            .args(args)
            .output()
            .expect("Failed to run tests");

        if !output.status.success() {
            return TestOutcome::Failure {
                details: [output.stderr, output.stdout].concat(),
            };
        }
    }

    // `cargo test` then
    {
        let args: Vec<OsString> = vec![
            "test".into(),
            "--manifest-path".into(),
            manifest_path.into(),
            "-q".into(),
            "--color".into(),
            color_option.into(),
        ];

        let output = std::process::Command::new("cargo")
            .args(args)
            .output()
            .expect("Failed to run tests");

        if !output.status.success() {
            return TestOutcome::Failure {
                details: [output.stderr, output.stdout].concat(),
            };
        }
    }

    TestOutcome::Success
}

#[derive(PartialEq)]
enum TestOutcome {
    Success,
    Failure { details: Vec<u8> },
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
