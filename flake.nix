{
  description = "Rust project";

  inputs = {
    harbor = {
      url = "git+https://github.com/caniko/harbor.git?ref=feat/harbor-monorepo-components&rev=e8b1996580e4ecd51dd402edf4a16d9df88e3a1b";
    };

    nixpkgs.follows = "harbor/nixpkgs";
    rust-overlay.follows = "harbor/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";

    treefmt-nix = {
      url = "github:numtide/treefmt-nix";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    git-hooks = {
      url = "github:cachix/git-hooks.nix";
      # The hook runner imports its own package set, including on Intel macOS.
      inputs.nixpkgs.follows = "harbor/nixpkgs-darwin";
    };
    plinth = {
      url = "git+https://github.com/caniko/plinth.git?ref=migrate/harbor-combined-20261012-features&rev=534dfa6dd6217db97b7ccd313501120a3c34d912";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = {
    self,
    nixpkgs,
    harbor,
    flake-utils,
    rust-overlay,
    treefmt-nix,
    git-hooks,
    plinth,
    ...
  }:
    flake-utils.lib.eachDefaultSystem (system: let
      pkgs =
        import (
          if system == "x86_64-darwin"
          then harbor.inputs.nixpkgs-darwin
          else nixpkgs
        ) {
          inherit system;
          overlays = [(import rust-overlay)];
        };

      toolchain = harbor.lib.rust.mkToolchain {
        inherit pkgs;
        toolchainProfile = "nightly";
        extensions = ["rustfmt" "clippy"];
        withRustAnalyzer = false;
        crossTargets = [];
      };
      cross = harbor.lib.rust.mkCross {inherit pkgs system;};
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
      packages = {
        default = package;
        codex-acp = codexAcpPackage;
        website = website;
        site = website;
      };
      apps.deploy-pages = plinth.lib.${system}.mkDeployPagesApp {
        domain = "visual-rubric.tartanoglu.com";
      };
      formatter = treefmtEval.config.build.wrapper;
      checks = {
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
      };
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

        docs = harbor.lib.rust.mkDocsShell {
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
