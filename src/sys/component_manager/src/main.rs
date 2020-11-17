// Copyright 2019 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

// TODO Follow 2018 idioms
#![allow(elided_lifetimes_in_paths)]

use {
    anyhow::{Context as _, Error},
    component_manager_lib::{
        builtin_environment::{BuiltinEnvironment, BuiltinEnvironmentBuilder},
        klog, startup,
    },
    fuchsia_async as fasync,
    fuchsia_runtime::{job_default, process_self},
    fuchsia_trace_provider as trace_provider,
    fuchsia_zircon::JobCriticalOptions,
    log::*,
    std::cell::Cell,
    std::{panic, process, thread, time::Duration},
};

fn main() -> Result<(), Error> {
    // Make sure we exit if there is a panic. Add this hook before we init the
    // KernelLogger because it installs its own hook and then calls any
    // existing hook.
    panic::set_hook(Box::new(|_| {
        println!("Panic in component_manager, aborting process.");
        // TODO remove after 43671 is resolved
        std::thread::spawn(move || {
            let mut nap_duration = Duration::from_secs(1);
            // Do a short sleep, hopefully under "normal" circumstances the
            // process will exit before this is printed
            thread::sleep(nap_duration);
            println!("component manager abort was started");
            // set a fairly long duration so we don't spam logs
            nap_duration = Duration::from_secs(30);
            loop {
                thread::sleep(nap_duration);
                println!("component manager alive long after abort");
            }
        });
        process::abort();
    }));

    // Set ourselves as critical to our job. If we do not fail gracefully, our
    // job will be killed.
    if let Err(err) =
        job_default().set_critical(JobCriticalOptions::RETCODE_NONZERO, &process_self())
    {
        panic!("Component manager failed to set itself as critical: {:?}", err);
    }

    klog::KernelLogger::init();

    info!("Component manager is starting up...");

    // Enable tracing in Component Manager
    trace_provider::trace_provider_create_with_fdio();

    // The following usage of `std::cell::Cell` is brought by the fact that
    // we'd like to configure the number of threads to spawn for our executor.
    // Unfortunately, the configured value lives in the config files that are
    // parsed by `BuiltinEnvironmentBuilder`. This is problematic because
    // *building* this object is asynchronous, and thus needs to run in an
    // async context. So we first need to build the `BuiltinEnvironment`
    // synchronously, and then extract the configured number of threads into
    // a local variable before running the root environment using the executor.
    // TODO(64534): Fix this dance.
    let builtin_environment: Cell<Option<BuiltinEnvironment>> = Cell::new(None);
    let mut executor = fasync::Executor::new().context("error creating executor")?;

    executor.run_singlethreaded(async {
        match build_environment().await {
            Ok(environment) => {
                builtin_environment.set(Some(environment));
            }
            Err(error) => {
                panic!("Component manager setup failed: {:?}", error);
            }
        }
    });

    let builtin_environment = builtin_environment.take().unwrap();

    // We store a copy of `num_threads` here so that we can safely move
    // `builtin_environment` into async block below.
    let num_threads = builtin_environment.num_threads;

    let fut = async move {
        if let Err(error) = builtin_environment.run_root().await {
            error!("Failed to bind to root component: {:?}", error);
            process::exit(1);
        }
    };

    executor.run(fut, num_threads);

    Ok(())
}

async fn build_environment() -> Result<BuiltinEnvironment, Error> {
    let args = match startup::Arguments::from_args() {
        Ok(args) => args,
        Err(err) => {
            error!("{}\n{}", err, startup::Arguments::usage());
            return Err(err);
        }
    };

    BuiltinEnvironmentBuilder::new()
        .set_args(args)
        .populate_config_from_args()
        .await?
        .create_utc_clock()
        .await?
        .add_elf_runner()?
        .include_namespace_resolvers()
        .build()
        .await
}
