use crate::protocol::{WorkerCommand, WorkerResponse, read_frame, write_frame};
use crate::shard::Shard;
use std::io;
use tokio::io::{AsyncRead, AsyncWrite, stdin, stdout};

pub async fn run_worker(id: usize) -> io::Result<()> {
    run_worker_stream(id, &mut stdin(), &mut stdout()).await
}

pub(crate) async fn run_worker_stream<R: AsyncRead + Unpin, W: AsyncWrite + Unpin>(
    id: usize,
    input: &mut R,
    output: &mut W,
) -> io::Result<()> {
    let mut worker = Shard::new(id);
    while let Some(command) = read_frame::<_, WorkerCommand>(input).await? {
        let shutdown = matches!(command, WorkerCommand::Shutdown);
        let response = worker
            .handle(command, output)
            .await
            .unwrap_or_else(WorkerResponse::Error);
        write_frame(output, &response).await?;
        if shutdown {
            break;
        }
    }
    Ok(())
}
