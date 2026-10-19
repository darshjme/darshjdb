// Compile against the package entry point, not its source tree.
import { DarshDB } from '..';
const client: DarshDB = new DarshDB('https://db.example.com');
void client;
