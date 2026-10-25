# Commercial B2B Agent Sample Calls & Testing Guide

This directory contains executable scripts and cURL specifications for testing the **Commercial B2B Agent** within the 4-Tier Enterprise Stack. It covers both **Mode A (Embedded Local Mode)** and **Mode B (Enterprise Identity Server Mode)**.

---

## 1. Prerequisites & Services Topology

| Service | Port | Description |
| :--- | :--- | :--- |
| **NATS JetStream** | `4222` | Event broker for DID cache (`did_web_cache`) and DLP synchronization |
| **Sovereign Identity Server** | `3075` | Enterprise IdP: dynamic tenant onboarding, client credential tokens, and JWKS |
| **B2B Agent (A2A)** | `4010` | Secure A2A JSON-RPC 2.0 server (`SecureAgentServer`) exposing `/` (`tasks/send`), `/agent-card`, `/skills` |
| **Embedded OAuth2 Server** | `4012` | Local dev fallback token issuer and JWKS |
| **Swarm Gateway (Tier 1)** | `8088` (or `8080`) | Cognitive Ingress Gateway (OpenAI ChatCompletions, Open Responses, Gemini format) |
| **Marketplace Trust Gateway (Tier 2)** | `8081` | Execution Egress Firewall validating tools (`claw_weather`), policies, dynamic RP lookup, and execution |

### Running the Services Locally
To test the full stack, start each service in a separate terminal:

```bash
# Terminal 1: NATS JetStream
nats-server -js

# Terminal 2: Sovereign Identity Server (Port 3075)
cd ../identity_server
cargo run -- --listen 127.0.0.1:3075 --db identity.sqlite

# Terminal 3: Swarm Gateway (Port 8088)
cd ../swarm_gateway
cargo run --release -p swarm_server -- --bind-address 0.0.0.0:8088

# Terminal 4: Marketplace Trust Gateway (Port 8081)
cd ../marketplace_trust_gateway/private
cargo run --release --bin market-trust-gateway-pro

# Terminal 5: Commercial B2B Agent (Port 4010)
cd ../commercial_b2b_agent
make run
```

---

## 2. Automated Test Execution

### Mode B: Enterprise Identity Server Mode (Recommended & Active)
Validates the complete production flow: dynamic tenant registration on port `3075`, client credential minting, inbound A2A signature verification, and Trust Gateway execution:

```bash
cd commercial_b2b_agent
./sample_calls/run_samples_with_identity_server.sh
```

### Mode A: Embedded Local Mode (Development Fallback)
Validates against the embedded OAuth2 token issuer on port `4012`:

```bash
cd commercial_b2b_agent
./sample_calls/run_samples.sh
```

---

## 3. Step-by-Step cURL Calls: Mode B (Enterprise Identity Server)

### Step 1: Agent Card & Skills Discovery
```bash
curl -s http://127.0.0.1:4010/agent-card | jq .
curl -s http://127.0.0.1:4010/skills | jq .
```

### Step 2: Dynamically Provision Tenant on Identity Server (`tenant_beta`)
```bash
curl -s -X POST http://127.0.0.1:3075/api/portal/tenants \
  -H "Content-Type: application/json" \
  -d '{
    "tenant_id": "tenant_beta",
    "name": "Enterprise Organization (tenant_beta)",
    "rp_id": "localhost",
    "rp_origin": "http://127.0.0.1:3075"
  }' | jq .
```

### Step 3: Provision OAuth2 Client Credentials
```bash
curl -s -X POST http://127.0.0.1:3075/api/portal/clients \
  -H "Content-Type: application/json" \
  -d '{
    "tenant_id": "tenant_beta",
    "client_id": "company-beta",
    "client_name": "Agent Runner Client (company-beta)",
    "client_secret": "secret123",
    "redirect_uris": ["http://localhost:3000/callback"],
    "allowed_scopes": ["b2b:query", "mcp:execute", "tools:list", "tools:call"]
  }' | jq .
```

### Step 4: Mint Access Token from Identity Server
```bash
TOKEN=$(curl -s -X POST http://127.0.0.1:3075/oauth/token \
  -H "Content-Type: application/json" \
  -d '{
    "grant_type": "client_credentials",
    "client_id": "company-beta",
    "client_secret": "secret123",
    "scope": "b2b:query mcp:execute"
  }' | jq -r '.access_token // .token')

echo "Acquired Bearer Token: $TOKEN"
```

### Step 5: Weather Inquiry in Zagreb (Invokes `claw_weather`)
Send the A2A `tasks/send` JSON-RPC message:
```bash
curl -X POST http://127.0.0.1:4010/ \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "jsonrpc": "2.0",
    "id": "req-weather-1",
    "method": "tasks/send",
    "params": {
      "id": "task-zagreb-weather-1",
      "message": {
        "messageId": "msg-weather-1",
        "role": "user",
        "kind": "message",
        "parts": [
          {
            "kind": "text",
            "text": "What is the weather like in Zagreb ?"
          }
        ],
        "metadata": {
          "agent_jwt": "'"$TOKEN"'",
          "tenant_id": "tenant_beta"
        }
      },
      "historyLength": 50
    }
  }' | jq .
```

**Expected Flow:**
1. **Inbound Validation**: Commercial B2B Agent verifies `$TOKEN` against `B2B_JWKS_URL` (`http://127.0.0.1:3075/.well-known/jwks.json`).
2. **Cognitive Ingress**: Agent sends intent to Swarm Gateway (`:8088`), which selects `claw_weather` with `{"location": "Zagreb"}`.
3. **Execution Egress**: Agent sends MCP `tools/call` to Trust Gateway (`:8081`) with `TRUST_GATEWAY_API_KEY` and `X-Tenant-ID: tenant_beta`.
4. **Dynamic RP Verification**: Trust Gateway dynamically resolves `tenant_beta` from Identity Server (`:3075/api/portal/tenants`), validates claims, and executes the sandboxed tool.
5. **Synthesis**: Result is returned to Swarm Gateway, and the final synthesized answer is sent back to the caller.

---

### Step 6: Semantic Interaction Contract / Reputation Inquiry
```bash
curl -X POST http://127.0.0.1:4010/ \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "jsonrpc": "2.0",
    "id": "req-neg-1",
    "method": "tasks/send",
    "params": {
      "id": "task-negotiate-1",
      "message": {
        "messageId": "msg-neg-1",
        "role": "user",
        "kind": "message",
        "parts": [
          {
            "kind": "text",
            "text": "What are your services and limits under the Negotiated Interaction Contract Protocol?"
          }
        ],
        "metadata": {
          "agent_jwt": "'"$TOKEN"'",
          "tenant_id": "tenant_beta"
        }
      },
      "historyLength": 50
    }
  }' | jq .
```

