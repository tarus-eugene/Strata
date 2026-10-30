use super::*;
use std::{collections::HashMap, process::Stdio};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, BufReader},
    process::Command,
};

impl Manager {
    pub(super) async fn execute_video(&self, job: &Arc<Job>) -> Result<()> {
        let d = job.data.lock().unwrap().clone();
        let executable = tools::executable("yt-dlp")?;
        let content = d.parts_dir().join("content");
        fs::create_dir_all(&content)?;
        let temporary = d.parts_dir().join("temp");
        fs::create_dir_all(&temporary)?;
        let mut config = format!(
            "--no-playlist\n--continue\n--no-overwrites\n--no-simulate\n--windows-filenames\n--no-cache-dir\n--quiet\n--no-warnings\n--progress\n--newline\n--concurrent-fragments {}\n--paths {}\n--paths {}\n--output {}\n--progress-template {}\n--print {}\n",
            d.connections,
            tools::config_value(&format!("home:{}", content.display())),
            tools::config_value(&format!("temp:{}", temporary.display())),
            tools::config_value("%(title).120B [%(id)s].%(ext)s"),
            tools::config_value("download:STRATA_PROGRESS:%(progress)j"),
            tools::config_value("after_move:STRATA_FILE:%(filepath)j")
        );
        if let Ok(ffmpeg) = tools::executable("ffmpeg") {
            config += &format!(
                "--ffmpeg-location {}\n",
                tools::config_value(&ffmpeg.to_string_lossy())
            );
        }
        if let Ok(deno) = tools::executable("deno") {
            config += &format!(
                "--js-runtimes {}\n",
                tools::config_value(&format!("deno:{}", deno.display()))
            );
        } else if let Ok(node) = tools::executable("node") {
            config += &format!(
                "--js-runtimes {}\n",
                tools::config_value(&format!("node:{}", node.display()))
            );
        }
        if let Some(proxy) = &d.options.proxy {
            config += &format!(
                "--proxy {}\n",
                tools::config_value(proxy.connection_url()?.as_str())
            );
        }
        if let Some(username) = &d.options.username {
            config += &format!("--username {}\n", tools::config_value(username));
        }
        if let Some(password) = &d.options.password {
            config += &format!("--password {}\n", tools::config_value(&password.expose()?));
        }
        config += &format!("{}\n", tools::config_value(&d.url));
        let mut command = Command::new(&executable);
        command
            .args(["--ignore-config", "--config-locations", "-"])
            .env("TMP", &temporary)
            .env("TEMP", &temporary)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let (mut child, tree) = tools::spawn(&mut command).context("Cannot start yt-dlp")?;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(config.as_bytes()).await?;
        drop(stdin);
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut errors = child.stderr.take().unwrap().take(1024 * 1024);
        let diagnostics = tokio::spawn(async move {
            let mut bytes = Vec::new();
            errors.read_to_end(&mut bytes).await.map(|_| bytes)
        });
        let mut files = Vec::new();
        let mut progress = HashMap::new();
        job.data.lock().unwrap().status = Status::Downloading;
        loop {
            let mut line = String::new();
            let count = tokio::select! {
                result = output.read_line(&mut line) => result?,
                _ = stopped(job) => { drop(tree); let _ = child.kill().await; let _ = diagnostics.await; bail!("Transfer stopped"); },
            };
            if count == 0 {
                break;
            }
            ensure!(
                line.len() <= 64 * 1024,
                "yt-dlp returned an oversized progress record"
            );
            if let Some(json) = line.trim().strip_prefix("STRATA_PROGRESS:") {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
                    let file = value["filename"].as_str().unwrap_or("video").to_owned();
                    let downloaded = value["downloaded_bytes"].as_u64().unwrap_or(0);
                    progress.insert(file, downloaded);
                    job.data.lock().unwrap().downloaded = progress.values().sum();
                }
            } else if let Some(json) = line.trim().strip_prefix("STRATA_FILE:") {
                files.push(serde_json::from_str::<String>(json)?);
            }
        }
        let status = tokio::select! { result = child.wait() => result?, _ = stopped(job) => { drop(tree); let _ = child.kill().await; bail!("Transfer stopped"); } };
        drop(tree);
        let mut error = String::from_utf8_lossy(&diagnostics.await??).to_string();
        for secret in [
            d.options.password.as_ref(),
            d.options.proxy.as_ref().and_then(|p| p.password.as_ref()),
        ]
        .into_iter()
        .flatten()
        {
            secret.redact(&mut error)?;
        }
        error = error.chars().take(8192).collect();
        ensure!(status.success(), "yt-dlp failed: {}", error.trim());
        ensure!(
            !files.is_empty(),
            "yt-dlp did not report a completed media file"
        );
        let root = content.canonicalize()?;
        let mut size = 0;
        for file in files {
            let file = PathBuf::from(file).canonicalize()?;
            ensure!(
                file.starts_with(&root) && file.is_file(),
                "yt-dlp returned a file outside its download folder"
            );
            size += fs::metadata(file)?.len();
        }
        job.data.lock().unwrap().downloaded = size;
        tools::publish_directory(job, &content)
    }
}
