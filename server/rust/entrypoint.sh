#!/bin/bash
set -e

# Self-register public IP in Route 53 if configured
if [ -n "$ROUTE53_ZONE_ID" ] && [ -n "$ROUTE53_DNS_NAME" ]; then
  echo "Registering in Route 53..."

  # Get public IP from ECS metadata endpoint
  METADATA_URI="${ECS_CONTAINER_METADATA_URI_V4}"
  if [ -n "$METADATA_URI" ]; then
    TASK_METADATA=$(curl -s "${METADATA_URI}/task")
    ENI_ID=$(echo "$TASK_METADATA" | grep -o '"networkInterfaceId":"[^"]*"' | head -1 | cut -d'"' -f4)

    if [ -n "$ENI_ID" ]; then
      PUBLIC_IP=$(aws ec2 describe-network-interfaces \
        --network-interface-ids "$ENI_ID" \
        --query 'NetworkInterfaces[0].Association.PublicIp' \
        --output text 2>/dev/null)
    fi
  fi

  # Fallback: query external service
  if [ -z "$PUBLIC_IP" ] || [ "$PUBLIC_IP" = "None" ]; then
    PUBLIC_IP=$(curl -s --max-time 5 http://checkip.amazonaws.com || true)
  fi

  if [ -n "$PUBLIC_IP" ] && [ "$PUBLIC_IP" != "None" ]; then
    echo "Public IP: $PUBLIC_IP"
    aws route53 change-resource-record-sets \
      --hosted-zone-id "$ROUTE53_ZONE_ID" \
      --change-batch "{
        \"Changes\": [{
          \"Action\": \"UPSERT\",
          \"ResourceRecordSet\": {
            \"Name\": \"$ROUTE53_DNS_NAME\",
            \"Type\": \"A\",
            \"TTL\": 60,
            \"ResourceRecords\": [{\"Value\": \"$PUBLIC_IP\"}]
          }
        }]
      }"
    echo "Registered $ROUTE53_DNS_NAME -> $PUBLIC_IP"
  else
    echo "WARNING: Could not determine public IP, skipping Route 53 registration"
  fi
fi

# Start the server
exec /app/tcpunchd