{
  description = "Rust project";

  inputs = {
    rs-harbor = {
      url = "git+https://codeberg.org/caniko/rs-harbor.git";
    };

    nixpkgs.follows = "rs-harbor/nixpkgs";
    rust-overlay.follows = "rs-harbor/rust-overlay";
    crane.follows = "rs-harbor/crane";
    flake-utils.follows = "rs-harbor/flake-utils";

    treefmt-nix.url = "github:numtide/treefmt-nix";
    git-hooks.url = "github:cachix/git-hooks.nix";
    plinth = {
      url = "git+https://codeberg.org/caniko/plinth.git?ref=refs/heads/trunk";
      inputs.nixpkgs.follows = "nixpkgs";
      inputs.flake-utils.follows = "flake-utils";
    };
  };

  outputs = {
    self,
    nixpkgs,
    rs-harbor,
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

      toolchain = rs-harbor.lib.mkToolchain {
        inherit pkgs;
        channel = "stable";
        extensions = ["rustfmt" "clippy"];
        withRustAnalyzer = false;
        crossTargets = [];
      };
      cross = rs-harbor.lib.mkCross {inherit pkgs system;};
      inherit (toolchain) craneLib;

      src = craneLib.cleanCargoSource ./.;
      commonArgs = {
        inherit src;
        strictDeps = true;
      };
      cargoArtifacts = craneLib.buildDepsOnly commonArgs;
      package = craneLib.buildPackage (commonArgs // {inherit cargoArtifacts;});
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
        website = website;
        site = website;
      };
      apps.deploy-pages = plinth.lib.${system}.mkDeployPagesApp {
        domain = "visual-rubric.tartanoglu.com";
      };
      formatter = treefmtEval.config.build.wrapper;
      checks = {
        default = package;
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

        docs = rs-harbor.lib.mkDocsShell {
          inherit pkgs cross;
          inherit (toolchain) craneLib;
          checks = self.checks.${system};
          packages = with pkgs; [
            plinth.packages.${system}.plinth-project
            pre-commit
            rust-analyzer
          ] ++ pre-commit-check.enabledPackages;
          extraShellHook = pre-commit-check.shellHook;
        };
      };
    });
}
