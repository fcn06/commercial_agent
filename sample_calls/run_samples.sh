#!/usr/bin/env bash
set -e

B2B_URL="${B2B_URL:-http://127.0.0.1:4010}"
OAUTH_URL="${OAUTH_URL:-http://127.0.0.1:4012}"
GATEWAY_URL="${GATEWAY_URL:-http://127.0.0.1:8081}"

echo "================================================================================"
echo "Commercial B2B Agent Sample Calls Runner"
echo "B2B Agent:     $B2B_URL"
echo "OAuth2 Server: $OAUTH_URL"
echo "Trust Gateway: $GATEWAY_URL"
echo "================================================================================"

echo ""
echo "--- 1. Discovery & Agent Manifest ---"
echo "[GET] $B2B_URL/agent-card"
curl -s "$B2B_URL/agent-card" | jq . 2>/dev/null || curl -s "$B2B_URL/agent-card"

echo ""
echo "[GET] $B2B_URL/skills"
curl -s "$B2B_URL/skills" | jq . 2>/dev/null || curl -s "$B2B_URL/skills"

echo ""
echo "--- 2. OAuth2 Token Issuance (Client Credentials Flow) ---"
echo "[POST] $OAUTH_URL/oauth/token"
TOKEN_RESPONSE=$(curl -s -X POST "$OAUTH_URL/oauth/token" \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "grant_type=client_credentials&client_id=company-beta&client_secret=secret123&scope=b2b:query mcp:execute")

echo "$TOKEN_RESPONSE" | jq . 2>/dev/null || echo "$TOKEN_RESPONSE"
TOKEN=$(echo "$TOKEN_RESPONSE" | jq -r '.access_token // .token // empty')

if [ -z "$TOKEN" ]; then
  echo "❌ Error: Could not extract access_token from response."
  exit 1
fi

echo "✅ Access Token successfully acquired: ${TOKEN:0:32}..."

echo ""
echo "================================================================================"
echo "--- 3. Tool Execution Query: Zagreb Weather (triggers 'claw_weather') ---"
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
          "tenant_id": "company-beta"
        }
      },
      "historyLength": 50
    }
  }')

echo "📥 B2B Agent Weather Response:"
echo "$WEATHER_RESPONSE" | jq . 2>/dev/null || echo "$WEATHER_RESPONSE"

echo ""
echo "================================================================================"
echo "--- 4. Semantic Negotiation / Contract Proposal Query ---"
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
          "tenant_id": "company-beta"
        }
      },
      "historyLength": 50
    }
  }')

echo "📥 B2B Agent Negotiation Response:"
echo "$NEGOTIATE_RESPONSE" | jq . 2>/dev/null || echo "$NEGOTIATE_RESPONSE"

echo ""
echo "================================================================================"
echo "🎉 All sample calls finished!"
echo "================================================================================"
