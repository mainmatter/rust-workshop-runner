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

/// Retrieve the path to the directory containing the exercises
/// for the current collection of workshop-runner.
pub fn get_exercises_dir() -> Result<PathBuf, anyhow::Error> {
    #[derive(serde::Deserialize, Debug)]
    /// The configuration for the current collection of workshop-runner.
    struct ExercisesConfig {
        /// The path to the directory containing the workshop-runner.
        exercises_dir: PathBuf,
    }

    let git_root_dir = get_git_repository_root_dir()
        .context("Failed to determine the root path of the current `git` repository")?;
    let exercises_config_path = git_root_dir.join(".wr.toml");
    let exercises_config = fs_err::read_to_string(&exercises_config_path).context(
        "Failed to read the configuration for the current collection of workshop-runner",
    )?;
    let exercises_config: ExercisesConfig = toml::from_str(&exercises_config).with_context(|| {
        format!(
            "Failed to parse the configuration at `{}` for the current collection of workshop-runner",
            exercises_config_path.to_string_lossy()
        )
    })?;

    if exercises_config.exercises_dir.is_absolute() {
        Ok(exercises_config.exercises_dir)
    } else {
        Ok(git_root_dir.join(exercises_config.exercises_dir))
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
        Ok(path.into())
    } else {
        Err(anyhow!(
            "Failed to determine the root path of the current `git` repository"
        ))
    }
}

pub struct ExerciseCollection {
    exercises_dir: PathBuf,
    connection: Connection,
    exercises: BTreeSet<Exercise>,
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
        let exercises: BTreeSet<Exercise> = chapters
            .flat_map(|entry| {
                let chapter_name = entry.file_name();
                read_dir(entry.path()).unwrap().map(move |f| {
                    let exercise = f.unwrap();
                    (chapter_name.to_owned(), exercise.file_name())
                })
            })
            .map(|(c, k)| Exercise::new(&c, &k))
            .collect::<Result<_, _>>()?;

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
    pub fn opened(&self) -> Result<BTreeSet<Exercise>, anyhow::Error> {
        opened_exercises(&self.connection)
    }

    /// Return the next exercise that should be opened, if we are going through the workshop-runner
    /// in the expected order.
    pub fn next(&self) -> Result<Option<Exercise>, anyhow::Error> {
        Ok(self
            .exercises
            .difference(&opened_exercises(&self.connection)?)
            .next()
            .cloned())
    }

    /// Open a specific exercise.
    pub fn open(&mut self, exercise: &Exercise) -> Result<(), anyhow::Error> {
        if !self.exercises.contains(exercise) {
            bail!("The exercise you are trying to open doesn't exist")
        }
        self.connection
            .execute(
                "INSERT OR IGNORE INTO open_exercises (chapter, exercise) VALUES (?1, ?2)",
                params![exercise.chapter(), exercise.exercise(),],
            )
            .context("Failed to open the next exercise")?;
        Ok(())
    }

    /// Open the next exercise, assuming we are going through the workshop-runner in order.
    pub fn open_next(&mut self) -> Result<Exercise, anyhow::Error> {
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
    pub fn iter(&self) -> impl Iterator<Item = &Exercise> {
        self.exercises.iter()
    }
}

/// Return the set of all workshop-runner that have been opened.
fn opened_exercises(connection: &Connection) -> Result<BTreeSet<Exercise>, anyhow::Error> {
    let err_msg = "Failed to retrieve the list of exercises that you have already started";
    let mut stmt = connection
        .prepare("SELECT chapter, exercise FROM open_exercises")
        .context(err_msg)?;
    let opened_exercises = stmt
        .query_map([], |row| {
            let chapter = row.get_ref_unwrap(0).as_str().unwrap();
            let exercise = row.get_ref_unwrap(1).as_str().unwrap();
            Ok(Exercise::new(chapter.as_ref(), exercise.as_ref())
                .expect("An invalid exercise has been stored in the database"))
        })
        .context(err_msg)?
        .collect::<Result<BTreeSet<_>, _>>()?;
    Ok(opened_exercises)
}

#[derive(Clone, PartialEq, Eq)]
pub struct Exercise {
    chapter_name: String,
    chapter_number: u16,
    name: String,
    number: u16,
}

impl PartialOrd for Exercise {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        let ord = self
            .chapter_number
            .cmp(&other.chapter_number)
            .then(self.number.cmp(&other.number));
        Some(ord)
    }
}

impl Ord for Exercise {
    fn cmp(&self, other: &Self) -> Ordering {
        self.partial_cmp(other).unwrap()
    }
}

impl Exercise {
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

        Ok(Exercise {
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

    /// The number+name of the chapter that contains this exercise.
    pub fn chapter(&self) -> String {
        format!("{:02}_{}", self.chapter_number, self.chapter_name)
    }

    /// The number+name of the this exercise.
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

impl std::fmt::Display for Exercise {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "({:02}) {} - ({:02}) {}",
            self.chapter_number, self.chapter_name, self.number, self.name
        )
    }
}
