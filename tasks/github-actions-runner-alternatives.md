# GitHub Actions Runner Alternatives - Cost Optimization

**Status:** Research Complete - Decision Pending
**Created:** 2025-11-07
**Context:** Investigating cheaper alternatives to GitHub Actions runners after experiencing rust-lld linker crashes and long build times

## Problem Statement

Current GitHub Actions setup:
- Standard 2-core runners ($0.008/min)
- ~10 minute builds for large Rust/DataFusion project
- Experiencing linker crashes (bus errors) due to memory constraints
- Cost: ~$4/month for 50 builds

## Cost Comparison Summary

For **10-minute Rust build**, **50 builds/month**:

| Provider | Cost/Build | Monthly Cost | vs GitHub | Setup Complexity |
|----------|------------|--------------|-----------|------------------|
| GitHub Standard | $0.080 | $4.00 | 1x | ✅ None |
| GitHub Large (4-core) | $0.160 | $8.00 | 2x | ✅ Easy |
| **RunsOn** | **$0.010** | **$0.50** | **0.125x** | ✅ Easy |
| **Ubicloud** | **$0.010** | **$0.50** | **0.125x** | ✅ Easy |
| **AWS EC2 Spot** | **$0.005** | **$0.25** | **0.06x** | 🔧 Medium |
| **AWS EC2 Spot (ARM)** | **$0.003** | **$0.15** | **0.04x** | 🔧 Medium |
| Namespace | $0.040 | $2.00 | 0.5x | ✅ Easy |
| BuildJet | $0.040 | $2.00 | 0.5x | ⚠️ Medium |
| **Hetzner Self-Hosted** | **$0.48** | **$24/mo flat** | varies | 🔧 Hard |

**Savings potential: 85-96%**

---

## Option 1: RunsOn (Recommended - Quick Win) ⭐

### Overview
- **Cost:** ~10x cheaper than GitHub
- **Performance:** 2x faster than GitHub standard runners
- **Setup:** Drop-in replacement, minimal changes
- **Backend:** AWS-backed with good caching

### Setup
```yaml
runs-on: runs-on,runner=8cpu-linux-x64  # 8 cores, 32GB RAM
```

### Pricing Details
- ~$0.001-0.002/min (vs GitHub $0.008/min)
- **$0.50/month for 50 builds**

### Pros
- Easiest to set up (5 minutes)
- Minimal workflow changes
- Fast performance
- Good support

### Cons
- Not the absolute cheapest option
- Less control than self-hosted

### Links
- Website: https://runs-on.com/
- Setup guide: https://runs-on.com/guides/getting-started/

---

## Option 2: AWS EC2 Spot Instances (Maximum Savings) 🏆

### Overview
- **Cost:** 60-90% cheaper than EC2 on-demand, 85-95% cheaper than GitHub
- **Best for:** Cost-sensitive workloads, ephemeral runners
- **Risk:** Spot interruptions (rare for short builds)

### Recommended Instance Types

| Instance | vCPU | RAM | Spot $/hr | Cost/Build | Why |
|----------|------|-----|-----------|------------|-----|
| **c7g.xlarge** 🏆 | 4 | 8GB | $0.036 | $0.006 | Best price/perf for Rust, ARM |
| **c7g.2xlarge** | 8 | 16GB | $0.072 | $0.012 | If linking needs more RAM |
| c6a.xlarge | 4 | 8GB | $0.055 | $0.009 | Intel x64 if needed |
| c6gd.xlarge | 4 | 8GB | $0.038 | $0.006 | ARM with NVMe for fast I/O |

**Recommendation:** `c7g.xlarge` (4-core ARM Graviton) - **$0.30/month for 50 builds**

### Setup Method A: GitHub Action (Easiest)

Use **machulav/ec2-github-runner**:

```yaml
name: Rust
on: [push]

jobs:
  build:
    runs-on: ubuntu-latest
    steps:
      - name: Start EC2 runner
        id: start-ec2-runner
        uses: machulav/ec2-github-runner@v2
        with:
          mode: start
          github-token: ${{ secrets.GH_PERSONAL_ACCESS_TOKEN }}
          ec2-image-id: ami-0c55b159cbfafe1f0  # Ubuntu 22.04 ARM
          ec2-instance-type: c7g.xlarge  # 4-core ARM
          subnet-id: subnet-xxxxx
          security-group-id: sg-xxxxx
          aws-resource-tags: >
            [
              {"Key": "Name", "Value": "github-runner"},
              {"Key": "GitHubRepository", "Value": "${{ github.repository }}"}
            ]
          spot-instance-only: true  # 🎯 Use spot!
          spot-instance-max-price: 0.05  # Set max price

      - name: Do the actual work
        uses: actions/checkout@v3
        run: |
          cd rust
          cargo build --release
          cargo test

      - name: Stop EC2 runner
        if: always()
        uses: machulav/ec2-github-runner@v2
        with:
          mode: stop
          github-token: ${{ secrets.GH_PERSONAL_ACCESS_TOKEN }}
          label: ${{ steps.start-ec2-runner.outputs.label }}
          ec2-instance-id: ${{ steps.start-ec2-runner.outputs.ec2-instance-id }}
```

**Prerequisites:**
1. AWS account with VPC configured
2. IAM role with EC2/VPC permissions
3. GitHub Personal Access Token (PAT)

**Setup time:** ~30 minutes

### Setup Method B: Terraform Module (Auto-scaling)

Use **github-aws-runners/terraform-aws-github-runner** (replaces archived philips-labs):

```hcl
module "github_runner" {
  source  = "github-aws-runners/github-runner/aws"
  version = "~> 5.0"

  github_app = {
    id         = var.github_app_id
    key_base64 = var.github_app_key_base64
  }

  # Spot configuration
  instance_target_capacity_type = "spot"
  instance_allocation_strategy  = "price-capacity-optimized"

  # ARM instances - cheapest and fast for Rust!
  instance_types = ["c7g.large", "c6g.large", "c6gd.large"]

  # Scale to zero when idle
  min_size = 0
  max_size = 10

  # Your workload
  runner_extra_labels = ["rust", "large-build"]
}
```

**Pros:**
- Auto-scales (0 cost when idle!)
- Survives spot interruptions (auto-replaces)
- Lambda orchestration included
- Production-ready

**Cons:**
- More complex setup
- Requires Terraform knowledge
- Overkill for simple use cases

**Setup time:** 1-2 hours

### Spot Instance Considerations

**Q: What if my instance gets interrupted?**
- GitHub Actions will retry automatically
- Use `price-capacity-optimized` allocation strategy (rarely interrupted)
- For 10-min builds, interruption risk is <1%

**Q: ARM vs x64 for Rust?**
- ARM (Graviton) is 40% cheaper and excellent for Rust
- Cross-compilation works well
- Native ARM Rust compilation is fast

**Q: Build cache?**
- Use S3 + actions/cache
- Or use Namespace cache (faster)
- Ephemeral runners mean fresh environment each time

### Cost Calculation Example

**Scenario:** 50 builds/month, 10 min average, c7g.xlarge spot

```
Cost = 50 builds × (10/60 hours) × $0.036/hour
     = 50 × 0.167 × $0.036
     = $0.30/month
```

**Savings:** $4.00 → $0.30 = **92.5% reduction!**

---

## Option 3: Hetzner Self-Hosted (DIY Maximum Savings)

### Overview
- **Cost:** Fixed monthly cost, unlimited builds
- **Best for:** High build frequency (>60 builds/month)
- **Setup:** More complex, requires management

### Recommended Server

**Hetzner CAX31** (ARM):
- 8 ARM cores (Ampere Altra)
- 16GB RAM
- 160GB NVMe SSD
- **Price:** €21.90/month (~$24/month)

Alternative: CPX31 (x64):
- 4 Intel vCPUs
- 8GB RAM
- 160GB NVMe SSD
- **Price:** €11.90/month (~$13/month)

### Cost Breakeven Analysis

**CAX31 @ $24/month:**
- Breakeven vs RunsOn: 48 builds/month ($0.50/build)
- Breakeven vs GitHub: 6 builds/month ($4/build)
- **Best for:** >50 builds/month

### Setup

Use **TestFlows Hetzner Runners**:

```yaml
- uses: testflows/testflows-github-hetzner-runners@v1
  with:
    server-type: cax31
    location: fsn1  # Falkenstein, Germany
    hetzner-token: ${{ secrets.HETZNER_TOKEN }}
```

Or manual setup:
1. Create Hetzner Cloud account
2. Deploy server with runner scripts
3. Register as self-hosted runner
4. Configure auto-scaling (optional)

### Pros
- Cheapest for high usage ($0.48/build @ 50 builds)
- Full control
- Great performance
- European data centers

### Cons
- You manage the server
- Hourly billing (charged for full hour even if 1 min job)
- Need to handle security, updates
- Single point of failure

### Links
- Hetzner Cloud: https://www.hetzner.com/cloud
- TestFlows Runners: https://github.com/testflows/testflows-github-hetzner-runners
- Manual setup guide: https://www.hetzner.com/cloud

---

## Option 4: Namespace (Premium Performance)

### Overview
- **Cost:** ~2x cheaper than GitHub
- **Best for:** Cache-heavy builds, large disk needs
- **Performance:** Best single-threaded x64 performance

### Key Features
- Up to 250GB disk storage
- Terabytes of cache storage
- High-performance caching via mounted volumes (faster than actions/cache)
- macOS runners on Apple Silicon (M4 Pro, M2 Pro)
- Remote terminal (SSH) and display (VNC) access

### Pricing
- ~$0.040/build (10 min)
- ~$2/month for 50 builds

### Setup
```yaml
runs-on: namespace-profile-rust-large
```

### Pros
- Excellent caching (critical for Rust/DataFusion)
- Best performance for x64
- More storage than competitors
- Good for heavy builds

### Cons
- 2x more expensive than RunsOn
- Overkill if you don't need cache/storage

### Links
- Website: https://namespace.so/
- Pricing: https://namespace.so/pricing

---

## Option 5: Ubicloud (Open Source Alternative)

### Overview
- **Cost:** ~10x cheaper than GitHub (similar to RunsOn)
- **Best for:** Open-source projects, cost-conscious
- **Backend:** Open-source cloud platform

### Pricing
- ~$0.001-0.002/min
- ~$0.50/month for 50 builds

### Setup
Similar to RunsOn - drop-in replacement.

### Pros
- Very cheap
- Open-source backing
- Simple setup

### Cons
- Newer platform (less track record)
- Fewer features than Namespace

### Links
- Website: https://www.ubicloud.com/
- GitHub: https://github.com/ubicloud/ubicloud

---

## Option 6: BuildJet

### Overview
- **Cost:** ~2x cheaper than GitHub
- **Backend:** Hetzner infrastructure

### Pricing
- ~$0.040/build
- ~$2/month for 50 builds

### Key Features
- 64GiB disk storage
- 20GiB free cache per repository via buildjet/cache
- Max 64 concurrent AMD vCPUs, 32 concurrent ARM vCPUs
- $300/month for 100 more concurrent vCPUs

### Caveats
- Network can be slow (must use their cache action)
- Cache complexity vs competitors

### Links
- Website: https://buildjet.com/
- Docs: https://buildjet.com/for-github-actions

---

## Decision Matrix

### Choose RunsOn if:
- ✅ You want quick setup (5 minutes)
- ✅ You want 10x cost savings with minimal effort
- ✅ You run <100 builds/month
- ✅ You don't want to manage infrastructure

### Choose AWS EC2 Spot if:
- ✅ You want maximum savings (92%+)
- ✅ You're comfortable with AWS
- ✅ You want auto-scaling
- ✅ You can tolerate rare interruptions

### Choose Hetzner Self-Hosted if:
- ✅ You run >50 builds/month
- ✅ You want absolute lowest cost
- ✅ You're comfortable managing servers
- ✅ You want European infrastructure

### Choose Namespace if:
- ✅ Your builds are cache-heavy (they are!)
- ✅ You need lots of storage (>250GB)
- ✅ You want best performance
- ✅ Cost is secondary to speed

### Stay with GitHub if:
- ✅ Cost doesn't matter
- ✅ You want zero management
- ✅ You need official support

---

## Recommended Action Plan

### Phase 1: Quick Win (This Week)
1. **Try RunsOn** first
   - Setup time: 5 minutes
   - Cost: $0.50/month
   - Easy to revert if issues
   - [Setup Guide](https://runs-on.com/guides/getting-started/)

### Phase 2: Evaluate (Next Week)
2. **Monitor RunsOn performance**
   - Build times
   - Reliability
   - Cost tracking

### Phase 3: Optimize (If Needed)
3. **If builds are still slow:**
   - Try Namespace (better caching)

4. **If builds are fast but need more savings:**
   - Migrate to AWS EC2 Spot (92% savings)

5. **If high build frequency (>60/month):**
   - Consider Hetzner self-hosted

---

## Additional Resources

### AWS IAM Policy for EC2 Spot Runners
```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "ec2:RunInstances",
        "ec2:TerminateInstances",
        "ec2:DescribeInstances",
        "ec2:DescribeInstanceStatus",
        "ec2:CreateTags",
        "ec2:RequestSpotInstances",
        "ec2:CancelSpotInstanceRequests",
        "ec2:DescribeSpotInstanceRequests"
      ],
      "Resource": "*"
    }
  ]
}
```

### Monitoring & Cost Tracking
- Use AWS Cost Explorer for EC2 costs
- GitHub Actions usage dashboard
- Set up billing alerts

### Links
- GitHub Actions Pricing: https://docs.github.com/en/billing/reference/actions-runner-pricing
- RunsOn: https://runs-on.com/
- AWS EC2 Spot Pricing: https://aws.amazon.com/ec2/spot/pricing/
- Hetzner Cloud: https://www.hetzner.com/cloud
- Namespace: https://namespace.so/
- Terraform AWS GitHub Runner: https://github.com/github-aws-runners/terraform-aws-github-runner

---

## Current Status

- ✅ Linker crash fixed (switched to gold linker)
- ✅ Added disk space cleanup in GitHub Actions
- ✅ Reduced parallel jobs to 2
- ⏳ Decision pending on runner alternatives
- 📊 Monitoring current build performance

## Next Steps

1. Wait for current CI build to complete
2. Evaluate if linker fixes resolved the issues
3. If builds are stable: decide on cost optimization
4. If builds still problematic: prioritize Namespace (better resources) or AWS EC2 spot (more RAM)
