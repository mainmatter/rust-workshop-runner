use anyhow::{anyhow, bail, Context};
use fs_err::read_dir;
use regex::Regex;
use rusqlite::{params, Connection};
use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::fmt::Formatter;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(serde::Deserialize, Debug)]
/// The configuration for the current collection of workshop-runner.
pub struct ExercisesConfig {
    /// The path to the directory containing the workshop-runner.
    #[serde(default = "default_exercise_dir")]
    exercises_dir: PathBuf,
    /// The command that should be run to verify that the workshop-runner is working as expected.
    #[serde(default)]
    verification: Vec<Verification>,
}

#[derive(serde::Deserialize, Debug)]
/// The configuration for a specific exercise.
pub struct ExerciseConfig {
    /// The commands that should be run to verify this exercise.
    /// It overrides the verification command specified in the collection configuration, if any.
    #[serde(default)]
    pub verification: Vec<Verification>,
}

#[derive(Debug, serde::Deserialize)]
pub struct Verification {
    /// The command that should be run to verify that the workshop-runner is working as expected.
    pub command: String,
    /// The arguments that should be passed to the verification command.
    #[serde(default)]
    pub args: Vec<String>,
}

fn default_exercise_dir() -> PathBuf {
    PathBuf::from("exercises")
}

impl ExercisesConfig {
    pub fn load() -> Result<Self, anyhow::Error> {
        let exercises_config_path = get_git_repository_root_dir()
            .context("Failed to determine the root path of the current `git` repository")?
            .join(".wr.toml");
        let exercises_config = fs_err::read_to_string(&exercises_config_path).context(
            "Failed to read the configuration for the current collection of workshop-runner",
        )?;
        let exercises_config: ExercisesConfig = toml::from_str(&exercises_config).with_context(|| {
            format!(
                "Failed to parse the configuration at `{}` for the current collection of workshop-runner",
                exercises_config_path.to_string_lossy()
            )
        })?;
        Ok(exercises_config)
    }

    /// The path to the directory containing the exercises
    /// for the current collection of workshop-runner.
    pub fn exercises_dir(&self) -> &Path {
        &self.exercises_dir
    }

    /// The command(s) that should be run to verify that exercises are correct.
    /// If empty, workshop-runner will use `cargo test` as default.
    pub fn verification(&self) -> &[Verification] {
        &self.verification
    }
}

/// Retrieve the path to the root directory of the current `git` repository.
pub fn get_git_repository_root_dir() -> Result<PathBuf, anyhow::Error> {
    let cmd = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("Failed to run a `git` command (`git rev-parse --show-toplevel`) to determine the root path of the current `git` repository")?;
    if cmd.status.success() {
        let path = String::from_utf8(cmd.stdout)
            .context("The root path of the current `git` repository is not valid UTF-8")?;
        Ok(path.trim().into())
    } else {
        Err(anyhow!(
            "Failed to determine the root path of the current `git` repository"
        ))
    }
}

pub struct ExerciseCollection {
    exercises_dir: PathBuf,
    connection: Connection,
    exercises: BTreeSet<ExerciseDefinition>,
}

impl ExerciseCollection {
    pub fn new(exercises_dir: PathBuf) -> Result<Self, anyhow::Error> {
        let chapters = read_dir(&exercises_dir)
            .context("Failed to read the workshop-runner directory")?
            .filter_map(|entry| {
                let Ok(entry) = entry else {
                    return None;
                };
                let Ok(file_type) = entry.file_type() else {
                    return None;
                };
                if file_type.is_dir() {
                    Some(entry)
                } else {
                    None
                }
            });
        let exercises: BTreeSet<ExerciseDefinition> = chapters
            .flat_map(|entry| {
                let chapter_name = entry.file_name();
                read_dir(entry.path()).unwrap().map(move |f| {
                    let exercise = f.unwrap();
                    (chapter_name.to_owned(), exercise.file_name())
                })
            })
            .filter_map(|(c, k)| ExerciseDefinition::new(&c, &k).ok())
            .collect();

        let db_path = exercises_dir.join("progress.db");
        // Open the database (or create it, if it doesn't exist yet).
        let connection = Connection::open(db_path)
            .context("Failed to create a SQLite database to track your progress")?;
        // Make sure all tables are initialised
        connection
            .execute(
                "CREATE TABLE IF NOT EXISTS open_exercises (
                chapter TEXT NOT NULL,
                exercise TEXT NOT NULL,
                solved INTEGER NOT NULL,
                PRIMARY KEY (chapter, exercise)
            )",
                [],
            )
            .context("Failed to initialise our SQLite database to track your progress")?;

        Ok(Self {
            connection,
            exercises_dir,
            exercises,
        })
    }

    pub fn n_opened(&self) -> Result<usize, anyhow::Error> {
        let err_msg = "Failed to determine how many workshop-runner have been opened";
        let mut stmt = self
            .connection
            .prepare("SELECT COUNT(*) FROM open_exercises")
            .context(err_msg)?;
        stmt.query_row([], |row| row.get(0)).context(err_msg)
    }

    /// Return an iterator over all the workshop-runner that have been opened.
    pub fn opened(&self) -> Result<BTreeSet<OpenedExercise>, anyhow::Error> {
        opened_exercises(&self.connection)
    }

    /// Return the next exercise that should be opened, if we are going through the workshop-runner
    /// in the expected order.
    pub fn next(&self) -> Result<Option<ExerciseDefinition>, anyhow::Error> {
        let opened = opened_exercises(&self.connection)?
            .into_iter()
            .map(|e| e.definition)
            .collect();
        Ok(self.exercises.difference(&opened).next().cloned())
    }

    /// Record in the database that an exercise was solved, so that it can be skipped next time.
    pub fn mark_as_solved(&self, exercise: &ExerciseDefinition) -> Result<(), anyhow::Error> {
        self.connection
            .execute(
                "UPDATE open_exercises SET solved = 1 WHERE chapter = ?1 AND exercise = ?2",
                params![exercise.chapter(), exercise.exercise(),],
            )
            .context("Failed to mark exercise as solved")?;
        Ok(())
    }

    /// Record in the database that an exercise was not solved, so that it won't be skipped next time.
    pub fn mark_as_unsolved(&self, exercise: &ExerciseDefinition) -> Result<(), anyhow::Error> {
        self.connection
            .execute(
                "UPDATE open_exercises SET solved = 0 WHERE chapter = ?1 AND exercise = ?2",
                params![exercise.chapter(), exercise.exercise(),],
            )
            .context("Failed to mark exercise as unsolved")?;
        Ok(())
    }

    /// Open a specific exercise.
    pub fn open(&mut self, exercise: &ExerciseDefinition) -> Result<(), anyhow::Error> {
        if !self.exercises.contains(exercise) {
            bail!("The exercise you are trying to open doesn't exist")
        }
        self.connection
            .execute(
                "INSERT OR IGNORE INTO open_exercises (chapter, exercise, solved) VALUES (?1, ?2, 0)",
                params![exercise.chapter(), exercise.exercise(),],
            )
            .context("Failed to open the next exercise")?;
        Ok(())
    }

    /// Open the next exercise, assuming we are going through the workshop-runner in order.
    pub fn open_next(&mut self) -> Result<ExerciseDefinition, anyhow::Error> {
        let Some(next) = self.next()? else {
            bail!("There are no more workshop-runner to open")
        };
        self.open(&next)?;
        Ok(next)
    }

    /// The directory containing all the workshop chapters and workshop-runner.
    pub fn exercises_dir(&self) -> &Path {
        &self.exercises_dir
    }

    /// Iterate over the workshop-runner in the collection, in the order we expect them to be completed.
    /// It returns both opened and unopened workshop-runner.
    pub fn iter(&self) -> impl Iterator<Item = &ExerciseDefinition> {
        self.exercises.iter()
    }
}

/// Return the set of all workshop-runner that have been opened.
fn opened_exercises(connection: &Connection) -> Result<BTreeSet<OpenedExercise>, anyhow::Error> {
    let err_msg = "Failed to retrieve the list of exercises that you have already started";
    let mut stmt = connection
        .prepare("SELECT chapter, exercise, solved FROM open_exercises")
        .context(err_msg)?;
    let opened_exercises = stmt
        .query_map([], |row| {
            let chapter = row.get_ref_unwrap(0).as_str().unwrap();
            let exercise = row.get_ref_unwrap(1).as_str().unwrap();
            let solved = row.get_ref_unwrap(2).as_i64().unwrap();
            let solved = if solved == 0 { false } else { true };
            let definition = ExerciseDefinition::new(chapter.as_ref(), exercise.as_ref())
                .expect("An invalid exercise has been stored in the database");
            Ok(OpenedExercise { definition, solved })
        })
        .context(err_msg)?
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(opened_exercises)
}

#[derive(Clone, PartialEq, Eq)]
pub struct ExerciseDefinition {
    chapter_name: String,
    chapter_number: u16,
    name: String,
    number: u16,
}

#[derive(Clone, PartialEq, Eq)]
pub struct OpenedExercise {
    pub definition: ExerciseDefinition,
    pub solved: bool,
}

impl PartialOrd for OpenedExercise {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.definition.partial_cmp(&other.definition)
    }
}

impl Ord for OpenedExercise {
    fn cmp(&self, other: &Self) -> Ordering {
        self.definition.cmp(&other.definition)
    }
}

impl PartialOrd for ExerciseDefinition {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let ord = self
            .chapter_number
            .cmp(&other.chapter_number)
            .then(self.number.cmp(&other.number));
        Some(ord)
    }
}

impl Ord for ExerciseDefinition {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}

impl PartialEq<OpenedExercise> for ExerciseDefinition {
    fn eq(&self, other: &OpenedExercise) -> bool {
        self == &other.definition
    }
}

impl PartialOrd<OpenedExercise> for ExerciseDefinition {
    fn partial_cmp(&self, other: &OpenedExercise) -> Option<Ordering> {
        self.partial_cmp(&other.definition)
    }
}

impl ExerciseDefinition {
    pub fn new(chapter_dir_name: &OsStr, exercise_dir_name: &OsStr) -> Result<Self, anyhow::Error> {
        fn parse(dir_name: &OsStr, type_: &str) -> Result<(String, u16), anyhow::Error> {
            // TODO: compile the regex only once.
            let re = Regex::new(r"(?P<number>\d{2})_(?P<name>\w+)").unwrap();

            let dir_name = dir_name.to_str().ok_or_else(|| {
                anyhow!(
                    "The name of a {type_} must be valid UTF-8 text, but {:?} isn't",
                    dir_name
                )
            })?;
            match re.captures(&dir_name) {
                None => bail!("Failed to parse `{dir_name:?}` as a {type_} (<NN>_<name>).",),
                Some(s) => {
                    let name = s["name"].into();
                    let number = s["number"].parse().unwrap();
                    Ok((name, number))
                }
            }
        }

        let (name, number) = parse(exercise_dir_name, "exercise")?;
        let (chapter_name, chapter_number) = parse(chapter_dir_name, "chapter")?;

        Ok(ExerciseDefinition {
            chapter_name,
            chapter_number,
            name,
            number,
        })
    }

    /// The path to the `Cargo.toml` file of the current exercise.
    pub fn manifest_path(&self, exercises_dir: &Path) -> PathBuf {
        self.manifest_folder_path(exercises_dir).join("Cargo.toml")
    }

    /// The path to the folder containing the `Cargo.toml` file for the current exercise.
    pub fn manifest_folder_path(&self, exercises_dir: &Path) -> PathBuf {
        exercises_dir.join(self.chapter()).join(self.exercise())
    }

    /// The configuration for the current exercise, if any.
    pub fn config(&self, exercises_dir: &Path) -> Result<Option<ExerciseConfig>, anyhow::Error> {
        let exercise_config = self.manifest_folder_path(exercises_dir).join(".wr.toml");
        if !exercise_config.exists() {
            return Ok(None);
        }
        let exercise_config = fs_err::read_to_string(&exercise_config).context(format!(
            "Failed to read the configuration for the exercise `{}`",
            self.exercise()
        ))?;
        let exercise_config: ExerciseConfig =
            toml::from_str(&exercise_config).with_context(|| {
                format!(
                    "Failed to parse the configuration for the exercise `{}`",
                    self.exercise()
                )
            })?;
        Ok(Some(exercise_config))
    }

    /// The number+name of the chapter that contains this exercise.
    pub fn chapter(&self) -> String {
        format!("{:02}_{}", self.chapter_number, self.chapter_name)
    }

    /// The number+name of this exercise.
    pub fn exercise(&self) -> String {
        format!("{:02}_{}", self.number, self.name)
    }

    /// The number of this exercise.
    pub fn exercise_number(&self) -> u16 {
        self.number
    }

    /// The number of the chapter that contains this exercise.
    pub fn chapter_number(&self) -> u16 {
        self.chapter_number
    }
}

impl std::fmt::Display for ExerciseDefinition {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({:02}) {} - ({:02}) {}",
            self.chapter_number, self.chapter_name, self.number, self.name
        )
    }
}
