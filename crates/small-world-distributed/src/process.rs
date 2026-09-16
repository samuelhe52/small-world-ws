use crate::protocol::{WorkerCommand, WorkerResponse, read_frame, write_frame};
use futures::future::try_join_all;
use std::process::Stdio;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

pub(crate) struct WorkerClient {
    child: Child,
    input: ChildStdin,
    output: ChildStdout,
}

impl WorkerClient {
    pub(crate) async fn spawn(id: usize) -> Result<Self, String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let mut child = Command::new(executable)
            .arg("worker")
            .arg("--id")
            .arg(id.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("failed to spawn worker {id}: {error}"))?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| format!("worker {id} has no stdin"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| format!("worker {id} has no stdout"))?;
        Ok(Self {
            child,
            input,
            output,
        })
    }

    pub(crate) async fn request(
        &mut self,
        command: WorkerCommand,
    ) -> Result<WorkerResponse, String> {
        self.send(command).await?;
        self.receive().await
    }

    pub(crate) async fn send(&mut self, command: WorkerCommand) -> Result<(), String> {
        write_frame(&mut self.input, &command)
            .await
            .map_err(|error| error.to_string())
    }

    pub(crate) async fn receive(&mut self) -> Result<WorkerResponse, String> {
        let response = read_frame(&mut self.output)
            .await
            .map_err(|error| error.to_string())?
            .ok_or_else(|| "worker closed its protocol stream".to_owned())?;
        match response {
            WorkerResponse::Error(error) => Err(error),
            response => Ok(response),
        }
    }

    pub(crate) async fn shutdown(mut self) {
        let _ = self.request(WorkerCommand::Shutdown).await;
        let _ = self.child.wait().await;
    }
}

pub(crate) async fn request_all(
    workers: &mut [WorkerClient],
    commands: Vec<WorkerCommand>,
) -> Result<Vec<WorkerResponse>, String> {
    if workers.len() != commands.len() {
        return Err("worker command count mismatch".to_owned());
    }
    try_join_all(
        workers
            .iter_mut()
            .zip(commands)
            .map(|(worker, command)| worker.request(command)),
    )
    .await
}
