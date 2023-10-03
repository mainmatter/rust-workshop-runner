# `wr`, a Rust workshop runner

> What I cannot create, I do not understand.
> 
> Richard Feynman

`wr` is a CLI to drive test-driven workshops written in Rust.  

It is designed to be used in conjunction with a workshop repository, which contains a series of exercises to be solved
by the workshop participants.

## How it works

A test-driven workshop is structured as a series of exercises.  
Each exercise is a Rust project with a set of tests that verify the correctness of the solution.  

`wr` will run the tests for the current exercise and, if they pass, allow you to move on to the next exercise while
keeping track of what you have solved so far.

You can see it in action in the [rust-telemetry-workshop](https://github.com/mainmatter/rust-telemetry-workshop).

## Installation

```bash
cargo install -f --path .

# Check that it has been installed correctly
wr --help
```

Run
```bash
wr
```
from the top-level folder of a workshop repository to verify your current solutions and move forward in the workshop.

Enjoy!

## Folder structure

`wr` expects the following structure for the workshop repository:

```
.
├── exercises
│  ├── 00_<collection name>
│  │  ├── 00_<exercise name>
│  │  │  ..
│  │  ├── 0n_<exercise name>
│  │  ..
│  ├── 0n_<collection name>
│  │  ├── 00_<exercise name>
│  │  │  ..
│  │  ├── 0n_<exercise name>
```

Each `xx_<exercise name>` folder must be a Rust project with its own `Cargo.toml` file.

You can choose a different top-level folder name by either passing the `--exercises-dir` flag when invoking `wr` 
or by creating a top-level `wr.toml` file with the following content:

```toml
exercises-dir = "my-top-level-folder"
```

You can refer to [rust-telemetry-workshop](https://github.com/mainmatter/rust-telemetry-workshop) as an example.

