use clap::{Args, Parser, Subcommand};
use rayon::ThreadPoolBuilder;
use small_world_threaded::{
    average_clustering_parallel, exact_path_length, generate_ws_partitioned, sample_sources,
    sampled_path_length_parallel, sampled_path_length_sequential,
};
use std::error::Error;
use std::time::{Duration, Instant};

#[derive(Debug, Parser)]
#[command(
    name = "small-world-ws",
    about = "Stage-6 Watts-Strogatz sampling and Rayon demo"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the large-network sampled/parallel analysis.
    Demo(DemoArgs),
    /// Compare sampled mean path length with the exact all-source value.
    Accuracy(AccuracyArgs),
    /// Start the browser dashboard and distributed-process coordinator.
    Serve(ServeArgs),
    /// Internal worker process entry point.
    #[command(hide = true)]
    Worker(WorkerArgs),
}

#[derive(Debug, Clone, Args)]
struct ServeArgs {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 8080)]
    port: u16,
}

#[derive(Debug, Clone, Args)]
struct WorkerArgs {
    #[arg(long)]
    id: usize,
}

#[derive(Debug, Clone, Args)]
struct DemoArgs {
    #[arg(long, default_value_t = 100_000)]
    nodes: usize,
    #[arg(long, default_value_t = 10)]
    degree: usize,
    #[arg(long, default_value_t = 0.05)]
    probability: f64,
    #[arg(long, default_value_t = 100)]
    samples: usize,
    #[arg(long, default_value_t = 4)]
    threads: usize,
    #[arg(long, default_value_t = 4)]
    partitions: usize,
    #[arg(long, default_value_t = 42)]
    seed: u64,
}

impl Default for DemoArgs {
    fn default() -> Self {
        Self {
            nodes: 100_000,
            degree: 10,
            probability: 0.05,
            samples: 100,
            threads: 4,
            partitions: 4,
            seed: 42,
        }
    }
}

#[derive(Debug, Clone, Args)]
struct AccuracyArgs {
    #[arg(long, default_value_t = 1_000)]
    nodes: usize,
    #[arg(long, default_value_t = 10)]
    degree: usize,
    #[arg(long, default_value_t = 0.05)]
    probability: f64,
    #[arg(long, default_value_t = 100)]
    samples: usize,
    #[arg(long, default_value_t = 4)]
    threads: usize,
    #[arg(long, default_value_t = 4)]
    partitions: usize,
    #[arg(long, default_value_t = 42)]
    seed: u64,
}

fn timed<T>(operation: impl FnOnce() -> T) -> (T, Duration) {
    let started = Instant::now();
    let value = operation();
    (value, started.elapsed())
}

fn run_demo(args: DemoArgs) -> Result<(), Box<dyn Error>> {
    if args.threads == 0 {
        return Err("threads must be at least 1".into());
    }
    let pool = ThreadPoolBuilder::new().num_threads(args.threads).build()?;

    let (generated, generation_time) = timed(|| {
        pool.install(|| {
            generate_ws_partitioned(
                args.nodes,
                args.degree,
                args.probability,
                args.partitions,
                args.seed,
            )
        })
    });
    let (graph, rewiring) = generated?;
    graph
        .validate()
        .map_err(|error| format!("invalid graph: {error}"))?;
    let sources = sample_sources(args.nodes, args.samples, args.seed ^ 0xb5ad_4ece);

    let (clustering, clustering_time) =
        timed(|| pool.install(|| average_clustering_parallel(&graph)));
    let (sequential, sequential_time) = timed(|| sampled_path_length_sequential(&graph, &sources));
    let (parallel, parallel_time) =
        timed(|| pool.install(|| sampled_path_length_parallel(&graph, &sources)));

    if sequential.reachable_pairs != parallel.reachable_pairs
        || sequential.mean_distance != parallel.mean_distance
    {
        return Err("parallel BFS result differs from sequential baseline".into());
    }

    let speedup = sequential_time.as_secs_f64() / parallel_time.as_secs_f64();
    let total = generation_time + clustering_time + sequential_time + parallel_time;
    println!("Small-world stage-6 demo");
    println!("nodes                  {}", graph.node_count());
    println!("edges                  {}", graph.edge_count());
    println!(
        "K / p                  {} / {:.6}",
        args.degree, args.probability
    );
    println!("logical partitions     {}", rewiring.partitions);
    println!("Rayon threads          {}", args.threads);
    println!(
        "rewired edges          {} / {}",
        rewiring.rewired_edges, rewiring.considered_edges
    );
    println!(
        "cross-partition updates {}",
        rewiring.cross_partition_updates
    );
    println!("sampled sources        {}", sources.len());
    println!("mean path length L     {:.6}", parallel.mean_distance);
    println!("mean clustering C      {:.6}", clustering);
    println!("generation             {:.3?}", generation_time);
    println!("parallel clustering    {:.3?}", clustering_time);
    println!("sequential sampled BFS {:.3?}", sequential_time);
    println!("parallel sampled BFS   {:.3?}", parallel_time);
    println!("BFS speedup            {:.2}x", speedup);
    println!("total measured work    {:.3?}", total);
    Ok(())
}

fn run_accuracy(args: AccuracyArgs) -> Result<(), Box<dyn Error>> {
    if args.threads == 0 {
        return Err("threads must be at least 1".into());
    }
    let pool = ThreadPoolBuilder::new().num_threads(args.threads).build()?;
    let (graph, _) = pool.install(|| {
        generate_ws_partitioned(
            args.nodes,
            args.degree,
            args.probability,
            args.partitions,
            args.seed,
        )
    })?;
    graph
        .validate()
        .map_err(|error| format!("invalid graph: {error}"))?;
    let sources = sample_sources(args.nodes, args.samples, args.seed ^ 0x1d2c_3b4a);
    let (exact, exact_time) = timed(|| exact_path_length(&graph));
    let (approximate, sampled_time) =
        timed(|| pool.install(|| sampled_path_length_parallel(&graph, &sources)));
    let relative_error = if exact.mean_distance == 0.0 {
        0.0
    } else {
        (approximate.mean_distance - exact.mean_distance).abs() / exact.mean_distance
    };

    println!("Sampling accuracy check");
    println!("nodes / samples        {} / {}", args.nodes, sources.len());
    println!("exact L                {:.6}", exact.mean_distance);
    println!("sampled L              {:.6}", approximate.mean_distance);
    println!("relative error         {:.3}%", relative_error * 100.0);
    println!("exact all-source time  {:.3?}", exact_time);
    println!("parallel sampled time  {:.3?}", sampled_time);
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Demo(DemoArgs::default())) {
        Command::Demo(args) => run_demo(args),
        Command::Accuracy(args) => run_accuracy(args),
        Command::Serve(args) => tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?
            .block_on(small_world_app::serve(&args.host, args.port)),
        Command::Worker(args) => {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?
                .block_on(small_world_distributed::run_worker(args.id))?;
            Ok(())
        }
    }
}
