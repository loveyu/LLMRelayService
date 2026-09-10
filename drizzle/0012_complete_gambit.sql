ALTER TABLE "console_requests" ADD COLUMN "initial_response_status" integer DEFAULT 0 NOT NULL;--> statement-breakpoint
ALTER TABLE "console_requests" ADD COLUMN "initial_response_status_text" text DEFAULT '' NOT NULL;--> statement-breakpoint
ALTER TABLE "console_requests" ADD COLUMN "initial_completed_at" bigint DEFAULT 0 NOT NULL;
--> statement-breakpoint
UPDATE "console_requests"
SET "initial_response_status" = CAST(SUBSTRING("failover_reason" FROM '^HTTP ([0-9]{3})$') AS integer),
    "initial_response_status_text" = "failover_reason",
    "original_route_prefix" = CASE
      WHEN POSITION(' (' IN "original_route_prefix") > 0
        THEN SPLIT_PART("original_route_prefix", ' (', 1)
      WHEN "original_request_model" IS NOT NULL
        AND RIGHT("original_route_prefix", LENGTH("original_request_model") + 1) = ':' || "original_request_model"
        THEN LEFT("original_route_prefix", LENGTH("original_route_prefix") - LENGTH("original_request_model") - 1)
      ELSE "original_route_prefix"
    END
WHERE "initial_response_status" = 0
  AND "response_status" >= 200
  AND "response_status" < 400
  AND "failover_reason" ~ '^HTTP [0-9]{3}$';
