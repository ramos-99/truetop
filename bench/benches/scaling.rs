use std::{hint::black_box, time::Duration};

use criterion::{
    AxisScale, BenchmarkId, Criterion, PlotConfiguration, criterion_group, criterion_main,
};
use truetop_bench::{PerCpuSample, collect_batched, collect_procfs};

const COUNTS: [usize; 5] = [100, 500, 1000, 5000, 10000];

fn scaling(c: &mut Criterion) {
    // Per-CPU slots per key, matching what the real collector folds: one slot per
    // CPU. Read from the host so the modelled fold cost reflects the machine the
    // benchmark runs on, not a hardcoded guess that would skew it elsewhere.
    let ncpus = std::thread::available_parallelism().map_or(8, |n| n.get());

    let mut group = c.benchmark_group("collect_cpu");
    group.plot_config(PlotConfiguration::default().summary_scale(AxisScale::Logarithmic));
    // The procfs arm is slow; fewer samples over a longer window keeps the
    // estimates tight without criterion bailing on the sample count.
    group.sample_size(50);
    group.measurement_time(Duration::from_secs(20));

    for &n in &COUNTS {
        let samples: Vec<(u32, PerCpuSample)> = (0..n as u32)
            .map(|pid| (pid, vec![u64::from(pid); ncpus]))
            .collect();
        group.bench_with_input(BenchmarkId::new("ebpf_batched", n), &samples, |b, s| {
            b.iter(|| collect_batched(black_box(s)));
        });
        group.bench_with_input(BenchmarkId::new("procfs_per_pid", n), &n, |b, &n| {
            b.iter(|| collect_procfs(black_box(n)));
        });
    }
    group.finish();
}

criterion_group!(benches, scaling);
criterion_main!(benches);
