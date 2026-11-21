-- What a device is, for the list's icon and model line.
ALTER TABLE devices ADD COLUMN kind TEXT NOT NULL DEFAULT 'computer';
ALTER TABLE devices ADD COLUMN model TEXT NOT NULL DEFAULT '';
