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
    permissions:
      contents: write

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
      env:
        RUSTDOCFLAGS: --html-in-header ../.github/doc-header.html

    - name: Prepare staging directory
      run: |
        mkdir -p public_docs/rust
        cp -r rust/target/doc/micromegas/* public_docs/rust/
        cp -r doc public_docs/

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

## Known Issues

*   **404 Errors for `doc` folder content:** The initial implementation caused 404 errors for files from the `doc` directory because the `doc` directory itself was not being copied into the deployment root.
    *   **Resolution:** The `cp` command in the workflow has been updated from `cp -r doc/* public_docs/` to `cp -r doc public_docs/` to ensure the correct path structure is maintained in the deployed pages. **(Resolved)**
*   **Broken Styling in Rust Documentation:** The generated Rustdoc pages do not render correctly (missing CSS and JS) when hosted in a subdirectory on GitHub Pages. This is because the HTML files use relative paths that break when the site is not at the root of the domain.
    *   **Resolution:** We will use `RUSTDOCFLAGS` to inject a `<base>` HTML tag into the generated documentation. This tag will specify the correct base URL for all relative paths, ensuring that the CSS, JavaScript, and other assets are loaded correctly. The base path will be set to `/micromegas/rust/`. **(Resolved)**
*   **Shell Parsing Errors with `RUSTDOCFLAGS`:** Passing the `<base>` tag directly in the workflow file can lead to shell parsing errors ("too many file operands").
    *   **Resolution:** The `<base>` tag is now stored in a separate file (`.github/doc-header.html`) and passed to `cargo doc` using the `--html-in-header` flag. This avoids any shell parsing issues. **(Resolved)**
*   **Rust Documentation Styling Still Broken (Persistent):** The styling of the Rust documentation is still broken because the shared CSS, JavaScript, and other static assets generated by `cargo doc` are located directly under `rust/target/doc/` (not within the `micromegas` subdirectory) and were not being copied to the correct location in the deployment.
    *   **Resolution:** The `cp` command for Rust documentation will be changed to `cp -r rust/target/doc/* public_docs/rust/`. This will copy all the contents of `rust/target/doc/` (including the shared assets and the `micromegas` subdirectory) into `public_docs/rust/`, ensuring all necessary files are present and correctly located relative to the `<base>` tag. **(Unresolved - Rustdoc build commented out)**
*   **Incorrect Rustdoc Asset Path:** Even after copying all the Rustdoc assets, the styling was still broken because the files were not being copied into the correct subdirectory (`rust`) in the final deployment.
    *   **Resolution:** The `cp` command in the workflow has been updated to `cp -r rust/target/doc public_docs/rust`, which correctly places all the Rustdoc files and assets in the `rust` subdirectory. **(Unresolved - Rustdoc build commented out)**
*   **404 Error on Rust Documentation (Persistent):** A previous fix for the asset path issue inadvertently broke the deployment by removing the `rust` subdirectory, leading to a 404 error for the entire Rust documentation.
    *   **Resolution:** The `cp` command has been corrected to `mkdir -p public_docs/rust && cp -r rust/target/doc/* public_docs/rust/`. This ensures the `rust` subdirectory is created and all the documentation files and assets are copied into it, resolving the 404 error and the styling issues. **(Unresolved - Rustdoc build commented out)**
*   **Rust Documentation Root 404 and Broken Internal Links (Persistent):** Navigating to `https://<username>.github.io/<repository-name>/rust/` results in a 404, and internal links within the Rust documentation are broken. This is because the `micromegas` crate's documentation is nested within `rust/target/doc/micromegas/`, and the previous copy command did not place it directly at the desired `/rust/` path in the deployed site.
    *   **Resolution:** The `cp` command for Rust documentation will be changed to copy the *contents* of `rust/target/doc/micromegas/` directly into `public_docs/rust/`. This will make `public_docs/rust/index.html` the main `micromegas` crate documentation. Additionally, the shared assets (CSS, JS, fonts) from `rust/target/doc/` will be copied into `public_docs/rust/` to ensure proper styling. **(Unresolved - Rustdoc build commented out)**
*   **Incorrect `cargo doc` Output Path:** `cargo doc` was generating documentation in the project root's `target/doc/` directory, not `rust/target/doc/`, even when `working-directory: rust/` was specified. This caused the workflow to look for files in the wrong location.
    *   **Resolution:** The `working-directory: rust/` will be removed from the `Build Rust documentation` step. The `cp` command for Rust documentation will be updated to copy from `target/doc/micromegas/` (relative to project root) directly into `public_docs/rust/`. **(Unresolved - Rustdoc build commented out)**
*   **`cargo doc` Requires `Cargo.toml` Context:** `cargo doc` needs to be run from a directory containing a `Cargo.toml` file (or a parent directory in a workspace). My previous attempt to remove `working-directory: rust/` was incorrect, as it caused `cargo doc` to fail.
    *   **Resolution:** `working-directory: rust/` will be re-added to the `Build Rust documentation` step. The `cp` command will then correctly copy from `rust/target/doc/micromegas/` (relative to the project root) into `public_docs/rust/`. **(Unresolved - Rustdoc build commented out)**

## Current Status

The Rust documentation build and publishing steps have been commented out in the workflow due to persistent issues with correct staging and serving. The `doc` folder content is still being published.