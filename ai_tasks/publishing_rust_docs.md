# Plan: Automate Rust Documentation Publishing to GitHub Pages

This document outlines the plan to automatically build and publish the Rust documentation for the `micromegas` crate to GitHub Pages whenever changes are merged into the `main` branch.

## Goal

Ensure that the latest Rust documentation is always available and accessible via GitHub Pages, reflecting the current state of the `main` branch.

## Proposed Solution: GitHub Actions Workflow

We will implement a new GitHub Actions workflow that triggers on pushes to the `main` branch, builds the documentation, and deploys it to the `gh-pages` branch.

### Workflow Details

**File Path:** `.github/workflows/publish-rust-docs.yml`

**Trigger:**

The workflow will be triggered on `push` events to the `main` branch.

```yaml
on:
  push:
    branches:
      - main
```

**Jobs:**

1.  **`build-and-deploy-docs` Job:**
    *   **Runner:** `ubuntu-latest`
    *   **Steps:**
        1.  **Checkout Code:** Use `actions/checkout@v4` to get the latest code.
        2.  **Setup Rust:** Use `dtolnay/rust-toolchain@stable` to install the Rust toolchain.
        3.  **Build Documentation:**
            *   Navigate to the `rust/` directory.
            *   Run `cargo doc -p micromegas --no-deps` to build the documentation for the `micromegas` crate. The `--no-deps` flag ensures only our crate's documentation is built, not its dependencies, which speeds up the process and reduces output size.
            *   The output will be in `rust/target/doc/micromegas/`.
        4.  **Deploy to GitHub Pages:**
            *   Use `peaceiris/actions-gh-pages@v4` to deploy the generated documentation.
            *   **Source Directory:** The `peaceiris/actions-gh-pages` action expects the documentation to be in a specific directory. We will configure it to look into `rust/target/doc/micromegas/`.
            *   **Target Branch:** `gh-pages` (this branch will be created automatically if it doesn't exist).
            *   **Authentication:** Use the default `GITHUB_TOKEN` provided by GitHub Actions.

### Example Workflow (`.github/workflows/publish-rust-docs.yml`)

```yaml
name: Publish Rust Docs

on:
  push:
    branches:
      - main

jobs:
  build-and-deploy-docs:
    runs-on: ubuntu-latest

    steps:
    - name: Checkout code
      uses: actions/checkout@v4

    - name: Setup Rust
      uses: dtolnay/rust-toolchain@stable
      with:
        toolchain: stable

    - name: Build Rust documentation
      run: cargo doc -p micromegas --no-deps
      working-directory: rust/

    - name: Deploy to GitHub Pages
      uses: peaceiris/actions-gh-pages@v4
      with:
        github_token: ${{ secrets.GITHUB_TOKEN }}
        publish_dir: rust/target/doc/micromegas/
        publish_branch: gh-pages
        force_orphan: true # Overwrite existing gh-pages branch content
```

## Setup Steps

1.  **Create Workflow File:** Create the file `.github/workflows/publish-rust-docs.yml` with the content provided above.
2.  **Configure GitHub Pages:**
    *   Go to your GitHub repository settings.
    *   Navigate to the "Pages" section.
    *   Under "Build and deployment", select "Deploy from a branch".
    *   Choose the `gh-pages` branch and select `/ (root)` as the folder.
    *   Click "Save".

## Verification

After implementing the workflow and configuring GitHub Pages:

1.  Merge a PR into the `main` branch.
2.  Observe the "Actions" tab in your GitHub repository to ensure the `Publish Rust Docs` workflow runs successfully.
3.  Once the workflow completes, check the GitHub Pages URL (usually `https://<username>.github.io/<repository-name>/micromegas/`) to confirm the documentation is published and accessible.

## Considerations

*   **`--no-deps`:** This flag is used to only build documentation for the `micromegas` crate itself, not its dependencies. This keeps the generated documentation focused and smaller.
*   **`force_orphan: true`:** This option in `peaceiris/actions-gh-pages` ensures that the `gh-pages` branch is completely overwritten with the new documentation, preventing stale files from previous builds.
*   **GitHub Token Permissions:** The default `GITHUB_TOKEN` usually has sufficient permissions to push to the `gh-pages` branch. If not, repository settings might need adjustment.
*   **Custom Domain:** If a custom domain is desired, it can be configured in the GitHub Pages settings after the initial setup.
