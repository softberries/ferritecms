// Zod's JIT builds validators with `new Function`, which the admin CSP
// (`script-src 'self'`, no `'unsafe-eval'`) blocks. This module runs the config
// before any schema exists: `main.tsx` imports it first, and every schema
// imports `z` from here (ESLint forbids importing `zod` anywhere else).
import { z } from "zod";

z.config({ jitless: true });

export { z };
