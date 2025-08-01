# Plan: Automate Rust and Project Documentation Publishing to GitHub Pages

This document outlines the plan to automatically build and publish the Rust documentation for the `micromegas` crate, along with the contents of the `doc` folder, to GitHub Pages whenever changes are merged into the `main` branch.

## Goal

Ensure that the latest Rust and project documentation is always available and accessible via GitHub Pages, reflecting the current state of the `main` branch.

## Proposed Solution: GitHub Actions Workflow

We will implement a new GitHub Actions workflow that triggers on pushes to the `main` branch, builds the documentation, stages it, and deploys it to the `gh-pages` branch.

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
        3.  **Build Rust Documentation:**
            *   Navigate to the `rust/` directory.
            *   Run `cargo doc -p micromegas --no-deps` to build the documentation for the `micromegas` crate.
        4.  **Prepare Staging Directory:**
            *   Create a staging directory named `public_docs`.
            *   Copy the generated Rust documentation from `rust/target/doc/micromegas/` into a `rust` subdirectory within `public_docs`.
            *   Copy the entire contents of the top-level `doc` directory into `public_docs`.
        5.  **Deploy to GitHub Pages:**
            *   Use `peaceiris/actions-gh-pages@v4` to deploy the generated documentation from the `public_docs` directory.
            *   **Target Branch:** `gh-pages`.
            *   **Authentication:** Use the default `GITHUB_TOKEN`.

### Example Workflow (`.github/workflows/publish-rust-docs.yml`)

```yaml
name: Publish Docs

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

    - name: Prepare staging directory
      run: |
        mkdir -p public_docs/rust
        cp -r rust/target/doc/micromegas/* public_docs/rust/
        cp -r doc/* public_docs/

    - name: Deploy to GitHub Pages
      uses: peaceiris/actions-gh-pages@v4
      with:
        github_token: ${{ secrets.GITHUB_TOKEN }}
        publish_dir: ./public_docs
        publish_branch: gh-pages
        force_orphan: true
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
2.  Observe the "Actions" tab in your GitHub repository to ensure the `Publish Docs` workflow runs successfully.
3.  Once the workflow completes, check the GitHub Pages URL. The Rust docs will be at `https://<username>.github.io/<repository-name>/rust/` and the other docs will be at their respective paths from the `doc` directory.

## Considerations

*   **Staging Directory:** A staging directory (`public_docs`) is used to combine the outputs from different sources before deploying.
*   **`force_orphan: true`:** This option in `peaceiris/actions-gh-pages` ensures that the `gh-pages` branch is completely overwritten with the new documentation, preventing stale files from previous builds.
*   **GitHub Token Permissions:** The default `GITHUB_TOKEN` usually has sufficient permissions to push to the `gh-pages` branch. If not, repository settings might need adjustment.