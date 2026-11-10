{
  description = "Rust project";

  inputs = {
    harbor-rs = {
      url = "git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev=0d53ee174d64cdebf22666691a7e8a99b2eb8cc6";
    };

    nixpkgs.follows = "harbor-rs/nixpkgs";
    rust-overlay.follows = "harbor-rs/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    plinth = {
      url = "git+https://github.com/caniko/plinth.git?ref=refs/heads/trunk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    harbor-rs,
    flake-utils,
    rust-overlay,
    treefmt-nix,
    git-hooks,
    plinth,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs = import nixpkgs {
        inherit system;
        overlays = [(import rust-overlay)];
      };

      toolchain = harbor-rs.lib.mkToolchain {
        inherit pkgs;
        toolchainProfile = "nightly";
        extensions = ["rustfmt" "clippy"];
        withRustAnalyzer = false;
        crossTargets = [];
      };
      cross = harbor-rs.lib.mkCross {inherit pkgs system;};
      inherit (toolchain) craneLib;
      # Verify the advertised minimum with its own compiler and artifacts.
      msrv = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.rust-version;
      msrvCraneLib = craneLib.overrideToolchain pkgs.rust-bin.stable."${msrv}.0".minimal;

      src = pkgs.lib.fileset.toSource {
        root = ./.;
        fileset = pkgs.lib.fileset.unions [
          (craneLib.fileset.commonCargoSources ./.)
          ./tests/fixtures
          ./README.md
          ./LICENSE
          ./CHANGELOG.md
        ];
      };
      commonArgs = {
        inherit src;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      msrvArgs = commonArgs // {cargoExtraArgs = "--all-features";};
      msrvArtifacts = msrvCraneLib.buildDepsOnly msrvArgs;
      package = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts;});
      codexAcpArgs =
        commonArgs
        // {
          cargoExtraArgs = "--features codex-acp";
        };
      codexAcpCargoArtifacts = craneLib.buildDepsOnly codexAcpArgs;
      codexAcpPackage = craneLib.buildPackage (codexAcpArgs
        // {
          cargoArtifacts = codexAcpCargoArtifacts;
        });
      # `cargo binstall visual-rubric` archives, built per target and published
      # by the release workflow. Linux only: every non-Linux target fails to
      # compile because `src/config.rs` calls `dirs::config_dir()` from a
      # `#[cfg(not(target_os = "linux"))]` branch and `dirs` is not a declared
      # dependency (E0433). Add that target list entry once it builds.
      release = harbor-rs.lib.mkBinstallRelease {
        inherit pkgs system;
        pname = "visual-rubric";
        version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
        inherit commonArgs;
        crates.visual-rubric = {cargoToml = ./Cargo.toml;};
        targets.x86_64-linux = ["x86_64-linux-musl" "aarch64-linux-musl"];
      };
      website = plinth.lib.${system}.mkProjectSite {
        pname = "visual-rubric-website";
        domain = "visual-rubric.tartanoglu.com";
        configPath = ./website/plinth-project.toml;
      };
      treefmtEval = treefmt-nix.lib.evalModule pkgs (import ./nix/treefmt.nix);
      pre-commit-check = git-hooks.lib.${system}.run {
        src = ./.;
        hooks = import ./nix/pre-commit.nix {
          inherit pkgs;
          treefmtWrapper = treefmtEval.config.build.wrapper;
          rustToolchain = toolchain.rustToolchain;
        };
      };
    in {
      packages =
        {
          default = package;
          codex-acp = codexAcpPackage;
          website = website;
          site = website;
        }
        // pkgs.lib.optionalAttrs (release.releaseBundle != null) {
          release-bundle = release.releaseBundle;
        };
      apps =
        {
          deploy-pages = plinth.lib.${system}.mkDeployPagesApp {
            domain = "visual-rubric.tartanoglu.com";
          };
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.hostPlatform.isLinux {
          # Simit prebuild publication on trunk (secret ATTIC_TOKEN). Linux
          # outputs only: darwin builds never reach the public canix cache.
          push-flake-inputs = harbor-rs.lib.mkAtticPush {
            inherit pkgs;
            flake = ".";
            paths = [package codexAcpPackage];
            adapter = harbor-rs.lib.mkAdapter {
              attic = {
                endpoint = "https://attic.candee.baby";
                cache = "canix";
                tokenEnvVar = "ATTIC_TOKEN";
              };
            };
          };
        };
      formatter = treefmtEval.config.build.wrapper;
      checks =
        {
          default = package;
          codex-acp = codexAcpPackage;
          formatting = treefmtEval.config.build.check self;
          tests-all-features = craneLib.cargoTest (commonArgs
            // {
              inherit cargoArtifacts;
              cargoTestExtraArgs = "--all-features";
            });
          msrv = msrvCraneLib.mkCargoDerivation (msrvArgs
            // {
              cargoArtifacts = msrvArtifacts;
              pname = "visual-rubric-msrv";
              buildPhaseCargoCommand = "cargo check --locked --offline --all-features --all-targets";
              doCheck = false;
            });
          docs = craneLib.cargoDoc (commonArgs
            // {
              inherit cargoArtifacts;
              cargoDocExtraArgs = "--all-features --no-deps";
              RUSTDOCFLAGS = "-D warnings";
            });
          package = craneLib.mkCargoDerivation (commonArgs
            // {
              inherit cargoArtifacts;
              pname = "visual-rubric-package";
              buildPhaseCargoCommand = "cargo package --locked --offline --allow-dirty --all-features";
              doCheck = false;
              installPhaseCommand = ''
                mkdir -p "$out"
                cp target/package/*.crate "$out/"
              '';
            });
          clippy = craneLib.cargoClippy (commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets --all-features -- --deny warnings";
            });
          clippy-no-default-features = craneLib.cargoClippy (commonArgs
            // {
              inherit cargoArtifacts;
              cargoClippyExtraArgs = "--all-targets --no-default-features -- --deny warnings";
            });
          fmt = craneLib.cargoFmt {inherit src;};
          # Fail if flake inputs ever point at the retired Codeberg/Codefloe
          # mirrors again (fleet migrated to github.com/caniko/*).
          # sourceUrl package metadata is excluded: informational only, not fetched.
          host-pinning = let
            # Split across literals so this file never matches its own pattern.
            staleHosts = "cod" + "eberg|cod" + "efloe";
          in
            pkgs.runCommand "visual-rubric-host-pinning" {} ''
              if ${pkgs.lib.getExe pkgs.ripgrep} -v "sourceUrl" ${./flake.nix} ${./flake.lock} \
                | ${pkgs.lib.getExe pkgs.ripgrep} -q "${staleHosts}"; then
                echo "ERROR: retired forge host in flake inputs:" >&2
                ${pkgs.lib.getExe pkgs.ripgrep} -v "sourceUrl" ${./flake.nix} ${./flake.lock} \
                  | ${pkgs.lib.getExe pkgs.ripgrep} -n "${staleHosts}" >&2 || true
                exit 1
              fi
              touch $out
            '';
        }
        // release.checks;
      devShells = {
        default = craneLib.devShell {
          checks = self.checks.${system};
          packages = with pkgs;
            [
              cargo-nextest
              pre-commit
              rust-analyzer
            ]
            ++ pre-commit-check.enabledPackages;
          shellHook = pre-commit-check.shellHook;
        };

        docs = harbor-rs.lib.mkDocsShell {
          inherit pkgs cross;
          inherit (toolchain) craneLib;
          checks = self.checks.${system};
          packages = with pkgs;
            [
              plinth.packages.${system}.plinth-project
              pre-commit
              rust-analyzer
            ]
            ++ pre-commit-check.enabledPackages;
          extraShellHook = pre-commit-check.shellHook;
        };
      };
    });
}
