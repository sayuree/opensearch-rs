/*
 * Licensed to Elasticsearch B.V. under one or more contributor
 * license agreements. See the NOTICE file distributed with
 * this work for additional information regarding copyright
 * ownership. Elasticsearch B.V. licenses this file to you under
 * the Apache License, Version 2.0 (the "License"); you may
 * not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *	http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing,
 * software distributed under the License is distributed on an
 * "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY
 * KIND, either express or implied.  See the License for the
 * specific language governing permissions and limitations
 * under the License.
 */

/*
 * SPDX-License-Identifier: Apache-2.0
 *
 * The OpenSearch Contributors require contributions made to
 * this file be licensed under the Apache-2.0 license or a
 * compatible open source license.
 *
 * Modifications Copyright OpenSearch Contributors. See
 * GitHub history for details.
 */

#![recursion_limit = "256"]

use clap::{App, Arg};
use log::{error, info, LevelFilter};
use reqwest::blocking::ClientBuilder;
use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::exit,
};

mod generator;
mod github;
mod regex;
mod skip;
mod step;

use generator::TestSuite;

use crate::skip::GlobalSkips;

fn main() -> anyhow::Result<()> {
    simple_logger::SimpleLogger::new()
        .with_level(LevelFilter::Info)
        .init()
        .unwrap();

    let matches = App::new(env!("CARGO_PKG_NAME"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .arg(Arg::with_name("url")
            .short("u")
            .long("url")
            .value_name("OPENSEARCH_URL")
            .help("The url of a running OpenSearch cluster. Used to determine the version, test suite and branch to use to compile tests")
            .required(true)
            .takes_value(true))
        .get_matches();

    let url = matches.value_of("url").expect("missing 'url' argument");
    let (branch, suite, version) = match branch_suite_and_version_from_opensearch(url) {
        Ok(v) => v,
        Err(e) => {
            error!("Problem getting values from OpenSearch at {}. {:?}", url, e);
            exit(1);
        }
    };

    info!("Using version {}", &version.to_string());
    info!("Using branch {}", &branch);
    info!("Using test_suite {:?}", &suite);

    let rest_specs_dir = Path::new("./api_generator/rest_specs");

    if !rest_specs_dir.exists()
        || rest_specs_dir
            .read_dir()
            .map(|mut e| e.next().is_none())
            .unwrap_or_else(|_| true)
    {
        error!(
            "No rest specs found at {}. Run api_generator project to download rest specs",
            rest_specs_dir.to_str().unwrap()
        );
        exit(1);
    }

    let last_downloaded_rest_spec_branch = rest_specs_dir.join("last_downloaded_version");

    if !last_downloaded_rest_spec_branch.exists() {
        error!(
            "No last downloaded rest version found at {}.",
            last_downloaded_rest_spec_branch.to_str().unwrap()
        );
        exit(1);
    }
    let rest_spec_version = fs::read_to_string(last_downloaded_rest_spec_branch)?;
    info!("Using rest specs from {}", &rest_spec_version);

    let download_dir = PathBuf::from(format!("./{}/yaml", env!("CARGO_PKG_NAME")));
    let generated_dir = PathBuf::from(format!("./{}/tests", env!("CARGO_PKG_NAME")));

    github::download_test_suites(&branch, &download_dir)?;

    let api = api_generator::generator::read_api(&branch, rest_specs_dir)?;

    // delete everything under the generated_dir except common dir
    if generated_dir.exists() {
        let entries = fs::read_dir(&generated_dir)?;
        for e in entries.flatten() {
            if let Ok(f) = e.file_type() {
                if e.file_name() != "common" {
                    if f.is_dir() {
                        fs::remove_dir_all(e.path())?;
                    } else if f.is_file() {
                        fs::remove_file(e.path())?;
                    }
                }
            }
        }
    }

    let global_skips = serde_yaml::from_str::<GlobalSkips>(include_str!("../skip.yml"))?;
    let skips = global_skips.get_skips_for(&version, url.starts_with("https://"));

    generator::generate_tests_from_yaml(
        &api,
        &suite,
        &version,
        &download_dir,
        &download_dir,
        &generated_dir,
        &skips,
    )?;

    Ok(())
}

fn branch_suite_and_version_from_opensearch(
    url: &str,
) -> anyhow::Result<(String, TestSuite, semver::Version)> {
    let client = ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .build()?;

    let suite = TestSuite::Free;

    let response = client.get(url).basic_auth("admin", Some("admin")).send()?;
    let json: Value = response.json()?;
    let branch = json["version"]["build_hash"].as_str().unwrap().to_string();

    // any prerelease part needs to be trimmed because the semver crate only allows
    // a version with a prerelease to match against predicates, if at least one predicate
    // has a prerelease. See
    // https://github.com/steveklabnik/semver/blob/afa5fc853cb4d6d2b1329579e5528f86f3b550f9/src/version_req.rs#L319-L331
    let version = json["version"]["number"]
        .as_str()
        .unwrap()
        .trim_end_matches(|c: char| c.is_alphabetic() || c == '-');

    Ok((branch, suite, semver::Version::parse(version)?))
}
