extern crate cmake;
use std::env;
use std::fs;
use std::path::{PathBuf, Path};
use std::process::Command;
use std::fs::File;
use std::io::prelude::*;

fn conan_build() {
	//let target = env::var("TARGET").unwrap();
	let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap();
	let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
	//let is_windows = if target_os == "windows" { true } else { false };

	Command::new("conan")
		.arg("install")
		.arg("-pr")
		.arg(format!("{}-{}", target_os, target_arch))
		.arg("conanfile.txt")
		.output()
		.expect("failed to execute process");

	let mut file = File::open("conanbuildinfo.cargo").expect("Error opening File");
	let mut contents = String::new();
	file.read_to_string(&mut contents).expect("Unable to read to string");
	println!("{}", contents);
}

fn main()
{
	if let Ok(_) = env::var("CARGO_FEATURE_CONAN_BUILD") {
		conan_build();
		return;
	}

	#[cfg(feature = "build-nng")]
	{
		let dst = cmake::Config::new("libnng")
			.define("NNG_TESTS", "OFF")
			.define("NNG_TOOLS", "OFF")
			.define("NNG_ENABLE_NNGCAT", "OFF")
			.define("NNG_ENABLE_COVERAGE", "OFF")
			.build();
		println!("cargo:rustc-link-search=native={}/lib", dst.display());
		println!("cargo:rustc-link-search=native={}/lib64", dst.display());
		println!("cargo:rustc-link-lib=static=nng");
	}
	#[cfg(not(feature = "build-nng"))]
	{
		println!("cargo:rustc-link-lib=dylib=nng");
	}
}
