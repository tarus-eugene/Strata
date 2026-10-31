use super::*;

impl Manager {
    pub(super) async fn execute_ftp(&self, job: &Arc<Job>) -> Result<()> {
        let d = job.data.lock().unwrap().clone();
        let curl = tools::executable("curl")?;
        let mut config = format!(
            "silent\nshow-error\nfail\nconnect-timeout = 15\nspeed-time = 30\nspeed-limit = 1\nftp-skip-pasv-ip\nproto = \"=ftp\"\nurl = {}\n",
            tools::config_value(&d.url)
        );
        if let Some(username) = &d.options.username {
            let password = d
                .options
                .password
                .as_ref()
                .map(|p| p.expose())
                .transpose()?
                .unwrap_or_default();
            config += &format!(
                "user = {}\n",
                tools::config_value(&format!("{username}:{password}"))
            );
        }
        if job.effective_speed_limit() > 0 {
            config += &format!("limit-rate = {}\n", job.effective_speed_limit());
        }
        let probe = tools::run(
            job,
            &curl,
            &tools::args(&["--disable", "--config", "-", "--head"]),
            config.as_bytes(),
            None,
        )
        .await?;
        let headers = String::from_utf8_lossy(&probe);
        let value = |name: &str| {
            headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.trim().to_owned())
        };
        let total = value("Content-Length")
            .map(|s| s.parse::<u64>())
            .transpose()?;
        let path = tools::begin_file(job, total, value("Last-Modified"))?;
        let offset = fs::metadata(&path)?.len();
        if total != Some(offset) || offset == 0 {
            config += &format!(
                "output = {}\ncontinue-at = {}\n",
                tools::config_value(&path.to_string_lossy()),
                offset
            );
            tools::run(
                job,
                &curl,
                &tools::args(&["--disable", "--config", "-"]),
                config.as_bytes(),
                Some(&path),
            )
            .await?;
        }
        tools::update_file_progress(job, fs::metadata(&path)?.len());
        tools::finish_file(job, &path).await
    }
}
