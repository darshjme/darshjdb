export default function hello(args: { name?: string }) {
  return { message: "Hello " + (args.name || "world") };
}
