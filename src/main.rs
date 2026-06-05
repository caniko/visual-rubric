use clap::Parser as _;

fn main() -> anyhow::Result<()> {
    visual_rubric::run(visual_rubric::Cli::parse())
}
