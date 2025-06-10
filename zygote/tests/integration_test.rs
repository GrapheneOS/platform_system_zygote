// Copyright (C) 2025 The Android Open-Source Project
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//      http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

#[cfg(not(soong))]
mod integration_test {
    use std::{
        env,
        io::{BufReader, Read},
        process::{Command, Stdio},
        thread::sleep,
        time::Duration,
    };

    use anyhow::{Context, Result};
    use rustix::path::Arg;
    use tempfile::tempdir;

    #[test]
    fn basic_test() -> Result<()> {
        let socket_dir = tempdir()?;
        let socket = socket_dir.path().join("socket");

        let zygote_cmd = env!("CARGO_BIN_EXE_zygote");
        let zygote_cli = env!("CARGO_BIN_EXE_zygote_cli");

        let mut zygote = Command::new(zygote_cmd)
            .arg("--socket")
            .arg(&socket)
            .args(["-v", "4", "--species", "lib-app", "--name", "zygote-server"])
            .stderr(Stdio::piped())
            .spawn()?;
        let stderr = zygote.stderr.take().context("Failed to get zygote STDERR")?;
        sleep(Duration::from_millis(250));

        let output = Command::new(zygote_cli)
            .arg("--socket")
            .arg(&socket)
            .args(["-v", "4", "IdentityQuery"])
            .output()?;
        assert!(output.status.success());

        let output = Command::new(zygote_cli)
            .arg("--socket")
            .arg(&socket)
            .args(["-v", "4", "Stat"])
            .output()?;
        assert!(output.status.success());

        let output = Command::new(zygote_cli)
            .arg("--socket")
            .arg(&socket)
            .args(["-v", "4", "Exit"])
            .output()?;
        assert!(output.status.success());

        let zygote_status = zygote.wait()?;
        assert!(zygote_status.success());

        let mut zygote_stderr = String::new();
        BufReader::new(stderr).read_to_string(&mut zygote_stderr)?;
        assert!(zygote_stderr.contains("Received client: (IdentityQuery"));
        assert!(zygote_stderr.contains("Received client: (Stat"));
        assert!(zygote_stderr.contains("Received client: (Exit"));

        Ok(())
    }

    #[test]
    fn spawn_lib_app() -> Result<()> {
        let socket_dir = tempdir()?;
        let socket = socket_dir.path().join("socket");

        let zygote_cmd = env!("CARGO_BIN_EXE_zygote");
        let zygote_cli = env!("CARGO_BIN_EXE_zygote_cli");
        let libmemmark =
            env::current_exe()?.parent().unwrap().parent().unwrap().join("libmemmark.so");

        let mut zygote = Command::new(zygote_cmd)
            .arg("--socket")
            .arg(&socket)
            .args(["-v", "4", "--species", "lib-app", "--name", "zygote-server"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdout = zygote.stdout.take().context("Failed to get zygote STDERR")?;
        let stderr = zygote.stderr.take().context("Failed to get zygote STDERR")?;
        sleep(Duration::from_millis(250));

        let output = Command::new(zygote_cli)
            .arg("--socket")
            .arg(&socket)
            .args(["-v", "4", "SpawnLibApp"])
            .arg(libmemmark)
            .args(["--", "--foo", "--bar"])
            .output()?;
        assert!(output.status.success());

        let output = Command::new(zygote_cli)
            .arg("--socket")
            .arg(&socket)
            .args(["-v", "4", "Exit"])
            .output()?;
        assert!(output.status.success());

        let zygote_status = zygote.wait()?;
        assert!(zygote_status.success());

        let mut zygote_stderr = String::new();
        BufReader::new(stderr).read_to_string(&mut zygote_stderr)?;
        assert!(zygote_stderr.contains("Successfully loaded shared library"));

        let mut zygote_stdout = String::new();
        BufReader::new(stdout).read_to_string(&mut zygote_stdout)?;
        assert!(zygote_stdout.contains("Hello from MemMark!"));
        assert!(zygote_stdout.contains(r#"Arguments: ["--foo", "--bar"]"#));

        Ok(())
    }

    #[test]
    fn launch() -> Result<()> {
        let zygote_launch = env!("CARGO_BIN_EXE_zygote_launch");
        let libmemmark =
            env::current_exe()?.parent().unwrap().parent().unwrap().join("libmemmark.so");

        let zygote_output = Command::new(zygote_launch)
            .args(["-v", "4", "--species", "lib-app"])
            .arg(libmemmark)
            .args(["--", "--foo", "--bar"])
            .output()?;
        assert!(zygote_output.status.success());

        assert!(zygote_output
            .stderr
            .to_string_lossy()
            .contains("Successfully loaded shared library"));

        assert!(zygote_output.stdout.to_string_lossy().contains("Hello from MemMark!"));
        assert!(zygote_output
            .stdout
            .to_string_lossy()
            .contains(r#"Arguments: ["--foo", "--bar"]"#));

        Ok(())
    }
}
