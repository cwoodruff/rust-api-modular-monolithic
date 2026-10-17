-- Port of Administration's AddCustomerPurchaseSummaryAndInbox migration: the
-- read model InvoiceFinalized feeds (ADR-0010), and the inbox that makes its
-- handler apply each event once (ADR-0008).

CREATE TABLE administration."CustomerPurchaseSummary" (
    "CustomerId" integer NOT NULL,
    "TotalSpent" numeric(12,2) NOT NULL,
    "InvoiceCount" integer NOT NULL,
    "LastPurchaseAt" timestamp with time zone NULL,
    CONSTRAINT "PK_CustomerPurchaseSummary" PRIMARY KEY ("CustomerId")
);

CREATE TABLE administration."InboxMessage" (
    "EventId" uuid NOT NULL,
    "HandlerName" varchar(300) NOT NULL,
    "ProcessedAt" timestamp with time zone NOT NULL,
    CONSTRAINT "PK_InboxMessage" PRIMARY KEY ("EventId", "HandlerName")
);
