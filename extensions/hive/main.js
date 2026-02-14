// Hive extension entrypoint
import { init } from "./lib/registry.js";

// Initialize the extension
try {
  await init();
} catch (e) {
  console.log(`[Hive] Error initializing extension: ${e.message}`);
}
