# Confidence Lambda Resolver Deployer

Docker-based deployer for the Confidence Lambda Resolver. Mirrors the Cloudflare deployer pattern: fetches resolver state from CDN, builds the Rust binary, and deploys to AWS Lambda. All infrastructure (IAM role, SQS queue, DynamoDB tables, Function URL) is auto-created.

## Quick Start

```bash
docker run --rm \
  -e AWS_ACCESS_KEY_ID="..." \
  -e AWS_SECRET_ACCESS_KEY="..." \
  -e AWS_REGION="eu-west-1" \
  -e CONFIDENCE_CLIENT_SECRET="..." \
  -e ENABLE_METRICS=1 \
  ghcr.io/spotify/confidence-lambda-deployer:latest
```

That's it. The deployer handles everything: IAM role, SQS queue, DynamoDB tables, Lambda functions, Function URL, and SQS-to-consumer wiring.

## Required Environment Variables

| Variable | Description |
|----------|-------------|
| `AWS_REGION` | AWS region for Lambda deployment |
| `CONFIDENCE_CLIENT_SECRET` | Confidence backend client secret (type BACKEND). Also used to derive the CDN state URL via SHA256. |

AWS credentials must be available — either via `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` env vars, or via an IAM instance role when running on EC2/ECS.

## Optional Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LAMBDA_FUNCTION_NAME` | `confidence-lambda-resolver` | Name for the resolver Lambda function |
| `CONSUMER_FUNCTION_NAME` | `confidence-lambda-consumer` | Name for the consumer Lambda function |
| `FUNCTION_NAME_PREFIX` | | Prefix for all resource names (e.g., `staging` produces `staging-confidence-lambda-resolver`) |
| `LAMBDA_MEMORY_MB` | `128` | Memory allocation for both Lambda functions |
| `LAMBDA_TIMEOUT` | `10` | Timeout in seconds for the resolver Lambda (consumer uses 30s) |
| `LAMBDA_ROLE_ARN` | (auto-created) | IAM role ARN for the Lambda functions. If not provided, a role is auto-created with the required permissions. |
| `CONFIDENCE_RESOLVER_STATE_URL` | (derived from client secret SHA256) | Override CDN URL for state download |
| `RESOLVE_TOKEN_ENCRYPTION_KEY` | | AES-128 base64-encoded key for resolve token encryption |
| `ALLOWED_ORIGIN` | `*` | CORS `Access-Control-Allow-Origin` value |
| `ENABLE_METRICS` | | Set to enable DynamoDB-backed Prometheus metrics and `/metrics` endpoint |
| `ENABLE_STICKY_ASSIGNMENTS_DYNAMODB` | | Set to create DynamoDB table for sticky assignments (materializations) |
| `FORCE_DEPLOY` | | Force re-deploy regardless of ETag/state changes |
| `NO_DEPLOY` | | Build only, skip deployment |

## Deployment Lifecycle

1. Validates AWS credentials via `sts:GetCallerIdentity`
2. Auto-creates IAM role if `LAMBDA_ROLE_ARN` not provided
3. Computes CDN URL from `SHA256(CONFIDENCE_CLIENT_SECRET)`
4. Checks deployed ETag via `aws lambda invoke` on the existing function
5. Downloads resolver state from CDN with `If-None-Match` (skips on 304)
6. Creates SQS queue if not exists
7. Creates DynamoDB tables if enabled and not exist
8. Builds Rust binary (single binary, dispatches via `HANDLER_MODE` env var)
9. Packages as Lambda zip (`bootstrap` binary)
10. Creates or updates both Lambda functions (same binary, different `HANDLER_MODE`)
11. Creates Function URL with public access permissions
12. Creates SQS event source mapping (batch 100, window 10s)

## ETag-Based Skip

Like the Cloudflare deployer, this script uses ETag-based conditional fetching. If the CDN returns 304 (state unchanged) and the deployer version hasn't changed, the script exits early without rebuilding or redeploying. This makes frequent cron invocations cheap.

## AWS Resources Created

| Resource | Name | Purpose |
|----------|------|---------|
| IAM role | `{name}-role` | Lambda execution permissions (SQS, DynamoDB, CloudWatch Logs) |
| Lambda function | `{prefix}-confidence-lambda-resolver` | HTTP resolver (Function URL) |
| Lambda function | `{prefix}-confidence-lambda-consumer` | SQS batch consumer |
| Function URL | (auto-generated) | Public HTTPS endpoint for the resolver |
| SQS queue | `{name}-flag-logs` | Flag log batching (equivalent to Cloudflare Queues) |
| SQS event source mapping | | Connects queue to consumer (batch 100, 10s window) |
| DynamoDB table (optional) | `{name}-metrics` | Prometheus metrics accumulation (equivalent to Cloudflare KV) |
| DynamoDB table (optional) | `{name}-materializations` | Sticky assignment storage (equivalent to Cloudflare KV) |

## IAM Permissions

If providing your own `LAMBDA_ROLE_ARN`, the role needs:

**Trust policy:**
```json
{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}
```

**Permissions:**
- `AWSLambdaBasicExecutionRole` (managed policy — CloudWatch Logs)
- `sqs:SendMessage`, `sqs:ReceiveMessage`, `sqs:DeleteMessage`, `sqs:GetQueueAttributes`
- `dynamodb:GetItem`, `dynamodb:PutItem`, `dynamodb:BatchGetItem`, `dynamodb:BatchWriteItem`, `dynamodb:Query` (if metrics or materializations enabled)

**Deployer credentials** (your AWS CLI auth) need:
- `lambda:*` on the function names
- `iam:CreateRole`, `iam:AttachRolePolicy`, `iam:PutRolePolicy`, `iam:GetRole` (if auto-creating role)
- `sqs:CreateQueue`, `sqs:GetQueueUrl`, `sqs:GetQueueAttributes`
- `dynamodb:CreateTable`, `dynamodb:DescribeTable` (if metrics or materializations enabled)
- `sts:GetCallerIdentity`

## Comparison with Cloudflare Deployer

| Aspect | Cloudflare | Lambda |
|--------|-----------|--------|
| Required inputs | `CLOUDFLARE_API_TOKEN` + `CONFIDENCE_CLIENT_SECRET` | `AWS_REGION` + `CONFIDENCE_CLIENT_SECRET` + AWS creds |
| Runtime | Wrangler CLI | AWS CLI |
| Queue | Auto-creates Cloudflare Queue | Auto-creates SQS queue |
| KV/Metrics | Auto-creates KV namespace | Auto-creates DynamoDB table |
| Auth | Cloudflare API token | IAM role (auto-created or provided) |
| Deploy | `wrangler deploy` | `aws lambda update-function-code` |
| Image | `ghcr.io/spotify/confidence-cloudflare-deployer` | `ghcr.io/spotify/confidence-lambda-deployer` |
