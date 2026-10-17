-- Port of Catalog's AddTrackSalesAndInbox migration: the read model
-- InvoiceFinalized feeds (ADR-0009), and the inbox that makes its handler
-- apply each event once (ADR-0008).

CREATE TABLE catalog."InboxMessage" (
    "EventId" uuid NOT NULL,
    "HandlerName" varchar(300) NOT NULL,
    "ProcessedAt" timestamp with time zone NOT NULL,
    CONSTRAINT "PK_InboxMessage" PRIMARY KEY ("EventId", "HandlerName")
);

CREATE TABLE catalog."TrackSales" (
    "TrackId" integer NOT NULL,
    "TimesSold" integer NOT NULL,
    "LastSoldAt" timestamp with time zone NULL,
    CONSTRAINT "PK_TrackSales" PRIMARY KEY ("TrackId")
);
