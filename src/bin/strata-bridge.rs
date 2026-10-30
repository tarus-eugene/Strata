use strata::{
    bridge::{self, Reply, Request},
    engine::history_path,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Only framed JSON goes to stdout; Chrome owns this process's standard streams.
    let reply = match run().await {
        Ok(reply) => reply,
        Err(error) => Reply::error(format!("{error:#}")),
    };
    if let Err(error) = bridge::write_message(&mut std::io::stdout().lock(), &reply) {
        eprintln!("{error:#}");
    }
}

async fn run() -> anyhow::Result<Reply> {
    let expected = format!("chrome-extension://{}/", bridge::EXTENSION_ID.trim());
    anyhow::ensure!(
        std::env::args().nth(1).as_deref() == Some(expected.as_str()),
        "This bridge only accepts the Strata browser extension"
    );
    let request = bridge::read_message(&mut std::io::stdin().lock())?;
    anyhow::ensure!(
        !matches!(request, Request::Transfer { .. }),
        "Advanced transfer options are available only through the local Strata CLI"
    );
    #[cfg(windows)]
    {
        bridge::send(
            &request,
            &history_path(),
            matches!(request, Request::Download { .. } | Request::Open),
        )
        .await
    }
    #[cfg(not(windows))]
    {
        let _ = request;
        anyhow::bail!("The Strata browser bridge currently supports Windows")
    }
}
