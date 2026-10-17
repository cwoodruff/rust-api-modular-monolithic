-- Port of Orders' AddInvoiceStatusAndOutbox migration: an invoice's status,
-- and the outbox InvoiceFinalized is published through (ADR-0008). Every
-- seeded invoice starts as Draft through the column default.

ALTER TABLE orders."Invoice" ADD "Status" varchar(20) NOT NULL DEFAULT 'Draft';

CREATE TABLE orders."OutboxMessage" (
    "Id" uuid NOT NULL,
    "EventType" varchar(300) NOT NULL,
    "Payload" jsonb NOT NULL,
    "OccurredAt" timestamp with time zone NOT NULL,
    "ProcessedAt" timestamp with time zone NULL,
    "Attempts" integer NOT NULL,
    "LastError" varchar(2000) NULL,
    "DeadLetteredAt" timestamp with time zone NULL,
    "NextAttemptAt" timestamp with time zone NOT NULL,
    CONSTRAINT "PK_OutboxMessage" PRIMARY KEY ("Id")
);

CREATE INDEX "IX_OutboxMessage_DeadLetteredAt" ON orders."OutboxMessage" ("DeadLetteredAt")
    WHERE "DeadLetteredAt" IS NOT NULL;
CREATE INDEX "IX_OutboxMessage_NextAttemptAt_OccurredAt" ON orders."OutboxMessage" ("NextAttemptAt", "OccurredAt")
    WHERE "ProcessedAt" IS NULL AND "DeadLetteredAt" IS NULL;
