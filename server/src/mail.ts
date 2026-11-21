// The only place that knows how mail is sent. To change provider, change this
// function (and the variables it reads); nothing else touches a mail service.

export interface MailEnv {
  MAIL_FROM: string;
  MAIL_PROVIDER?: string;
  RESEND_API_KEY?: string;
}

export async function sendMail(
  env: MailEnv,
  to: string,
  subject: string,
  text: string,
): Promise<void> {
  const provider = env.MAIL_PROVIDER ?? (env.RESEND_API_KEY ? "resend" : "log");
  if (provider === "resend") {
    const res = await fetch("https://api.resend.com/emails", {
      method: "POST",
      headers: {
        authorization: `Bearer ${env.RESEND_API_KEY}`,
        "content-type": "application/json",
      },
      body: JSON.stringify({ from: env.MAIL_FROM, to: [to], subject, text }),
    });
    if (!res.ok) throw new Error(`mail provider answered ${res.status}`);
    return;
  }
  // Log mode: for set-up only. The message shows in `wrangler tail`.
  console.log(`[mail:log] to=${to} subject=${subject}\n${text}`);
}
