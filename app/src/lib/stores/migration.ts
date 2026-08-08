/// One-time migration: rename all kepler-* localStorage keys to holt-*.
/// Called once on app init before any store initialization.
export function migrateLocalStorageKeys(): void {
    const prefix = 'kepler-';
    const newPrefix = 'holt-';
    try {
        const keysToMigrate: [string, string][] = [];
        for (let i = 0; i < localStorage.length; i++) {
            const key = localStorage.key(i);
            if (key && key.startsWith(prefix)) {
                const newKey = newPrefix + key.slice(prefix.length);
                const value = localStorage.getItem(key);
                if (value !== null && localStorage.getItem(newKey) === null) {
                    keysToMigrate.push([key, newKey]);
                }
            }
        }
        for (const [oldKey, newKey] of keysToMigrate) {
            const value = localStorage.getItem(oldKey);
            if (value !== null) {
                localStorage.setItem(newKey, value);
                localStorage.removeItem(oldKey);
            }
        }
    } catch {}
}
