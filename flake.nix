{
  description = "Rust project";

  inputs = {
    harbor-rs = {
      url = "git+https://github.com/caniko/harbor-rs.git?ref=trunk&rev=ed89d0b13fc61dd1b2217bf4bba97f32cec27ba7";
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
        # Fail if flake inputs ever point at the retired Codeberg/Codefloe
        # mirrors again (fleet migrated to github.com/caniko/*).
        # sourceUrl package metadata is excluded: informational only, not fetched.
        host-pinning =
          let
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
