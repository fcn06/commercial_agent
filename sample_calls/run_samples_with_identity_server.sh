#!/usr/bin/env bash
set -e

B2B_URL="${B2B_URL:-http://127.0.0.1:4010}"
IDENTITY_URL="${IDENTITY_URL:-http://127.0.0.1:3075}"
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:8081}"
TENANT_ID="${TENANT_ID:-tenant_beta}"
CLIENT_ID="${CLIENT_ID:-company-beta}"
CLIENT_SECRET="${CLIENT_SECRET:-secret123}"

echo "================================================================================"
echo "Commercial B2B Agent Sample Calls Runner (Enterprise Identity Server Mode)"
echo "B2B Agent:       $B2B_URL"
echo "Identity Server: $IDENTITY_URL"
echo "Trust Gateway:   $GATEWAY_URL"
echo "Tenant ID:       $TENANT_ID"
echo "Client ID:       $CLIENT_ID"
echo "================================================================================"

echo ""
echo "--- 1. Discovery & Agent Manifest ---"
echo "[GET] $IDENTITY_URL/.well-known/jwks.json"
curl -s "$IDENTITY_URL/.well-known/jwks.json" | jq . 2>/dev/null || curl -s "$IDENTITY_URL/.well-known/jwks.json"

echo ""
echo "[GET] $B2B_URL/agent-card"
curl -s "$B2B_URL/agent-card" | jq . 2>/dev/null || curl -s "$B2B_URL/agent-card"

echo ""
echo "[GET] $B2B_URL/skills"
curl -s "$B2B_URL/skills" | jq . 2>/dev/null || curl -s "$B2B_URL/skills"

echo ""
echo "================================================================================"
echo "--- 2. Ensure Tenant is Registered on Identity Server ($TENANT_ID) ---"
echo "================================================================================"
echo "[POST] $IDENTITY_URL/api/portal/tenants"
TENANT_REG_RESPONSE=$(curl -s -X POST "$IDENTITY_URL/api/portal/tenants" \
  -H "Content-Type: application/json" \
  -d '{
    "tenant_id": "'"$TENANT_ID"'",
    "name": "Enterprise Organization ('"$TENANT_ID"')",
    "rp_id": "localhost",
    "rp_origin": "'"$IDENTITY_URL"'"
  }' || true)
echo "$TENANT_REG_RESPONSE" | jq . 2>/dev/null || echo "$TENANT_REG_RESPONSE"

echo ""
echo "================================================================================"
echo "--- 3. Ensure OAuth Client is Registered on Identity Server ($CLIENT_ID) ---"
echo "================================================================================"
echo "[POST] $IDENTITY_URL/api/portal/clients"
CLIENT_REG_RESPONSE=$(curl -s -X POST "$IDENTITY_URL/api/portal/clients" \
  -H "Content-Type: application/json" \
  -d '{
    "tenant_id": "'"$TENANT_ID"'",
    "client_id": "'"$CLIENT_ID"'",
    "client_name": "Agent Runner Client ('"$CLIENT_ID"')",
    "client_secret": "'"$CLIENT_SECRET"'",
    "redirect_uris": ["http://localhost:3000/callback"],
    "allowed_scopes": ["b2b:query", "mcp:execute", "tools:list", "tools:call"]
  }' || true)
echo "$CLIENT_REG_RESPONSE" | jq . 2>/dev/null || echo "$CLIENT_REG_RESPONSE"

echo ""
echo "================================================================================"
echo "--- 4. OAuth2 Token Issuance via Identity Server (:3075) ---"
echo "================================================================================"
echo "[POST] $IDENTITY_URL/oauth/token"
TOKEN_RESPONSE=$(curl -s -X POST "$IDENTITY_URL/oauth/token" \
  -H "Content-Type: application/json" \
  -d '{
    "grant_type": "client_credentials",
    "client_id": "'"$CLIENT_ID"'",
    "client_secret": "'"$CLIENT_SECRET"'",
    "scope": "b2b:query mcp:execute"
  }')

echo "$TOKEN_RESPONSE" | jq . 2>/dev/null || echo "$TOKEN_RESPONSE"
TOKEN=$(echo "$TOKEN_RESPONSE" | jq -r '.access_token // .token // empty')

if [ -z "$TOKEN" ]; then
  echo "❌ Error: Could not extract access_token from response."
  exit 1
fi

echo "✅ Identity Server Access Token acquired: ${TOKEN:0:36}..."

echo ""
echo "================================================================================"
echo "--- 5. Tool Execution Query: Zagreb Weather (triggers 'claw_weather') ---"
echo "================================================================================"
echo "Prompt: What is the weather like in Zagreb ?"
echo "[POST] $B2B_URL/"

TASK_ID="task-weather-$(date +%s)-$RANDOM"
MSG_ID="msg-weather-$(date +%s)-$RANDOM"

WEATHER_RESPONSE=$(curl -s -X POST "$B2B_URL" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "jsonrpc": "2.0",
    "id": "req-weather-1",
    "method": "tasks/send",
    "params": {
      "id": "'"$TASK_ID"'",
      "message": {
        "messageId": "'"$MSG_ID"'",
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
          "tenant_id": "'"$TENANT_ID"'"
        }
      },
      "historyLength": 50
    }
  }')

echo "📥 B2B Agent Weather Response:"
echo "$WEATHER_RESPONSE" | jq . 2>/dev/null || echo "$WEATHER_RESPONSE"

echo ""
echo "================================================================================"
echo "--- 6. Semantic Negotiation / Contract Proposal Query ---"
echo "================================================================================"
echo "[POST] $B2B_URL/"

NEGOTIATE_ID="task-neg-$(date +%s)-$RANDOM"
NEGOTIATE_RESPONSE=$(curl -s -X POST "$B2B_URL" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{
    "jsonrpc": "2.0",
    "id": "req-neg-1",
    "method": "tasks/send",
    "params": {
      "id": "'"$NEGOTIATE_ID"'",
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
          "tenant_id": "'"$TENANT_ID"'"
        }
      },
      "historyLength": 50
    }
  }')

echo "📥 B2B Agent Negotiation Response:"
echo "$NEGOTIATE_RESPONSE" | jq . 2>/dev/null || echo "$NEGOTIATE_RESPONSE"

echo ""
echo "================================================================================"
echo "🎉 Enterprise Identity Server sample flow completed successfully!"
echo "================================================================================"
