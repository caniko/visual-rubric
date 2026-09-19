{
  description = "Rust project";

  inputs = {
    harbor-rs = {
      url = "git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev=fac8049316846e0ef1c1e6acd92aed7a337b333a";
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

      src = pkgs.lib.fileset.toSource {
        root = ./.;
        fileset = pkgs.lib.fileset.unions [
          (craneLib.fileset.commonCargoSources ./.)
          ./tests/fixtures
        ];
      };
      commonArgs = {
        inherit src;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
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
