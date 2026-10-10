/* eslint-disable @next/next/no-img-element */
import { Reveal } from "../Reveal";
import { SectionTitle, Body, Window } from "../landing";

export function ForHumans() {
  return (
    <section className="band mt-[clamp(8rem,18vh,11rem)] py-[clamp(3.5rem,8vh,5.5rem)]">
      <Reveal>
        <SectionTitle>humans</SectionTitle>
        <Body className="mt-8">See what needs you. Ignore the rest.</Body>
      </Reveal>
      <Reveal delay={100} className="mt-9 min-w-0">
        {/* Home in the browser and the same morning on a phone, cut from
            the HumansStill composition in promo/. From md up the phone
            hangs off the window's lower right corner; below md it stacks
            under the window, before the caption. */}
        <div className="grid min-w-0 md:grid-cols-[minmax(0,1fr)_auto] md:items-start xl:-mr-16">
          <div className="contents md:block md:min-w-0">
            <Window title="localhost:3456" className="min-w-0">
              <img
                src="/humans-home.webp"
                width={1760}
                height={1440}
                loading="lazy"
                alt="Lific Home on Thursday morning, greeting Blake. Needs you lists APP-51 Pick the launch date as assigned to him, APP-52 Sign the vendor contract as for any person, and APP-44 Approve retry limits as waiting on him. Below it, five active issues in App."
                className="block h-auto w-full"
              />
            </Window>
            <p className="order-3 mt-3 text-caption text-text-faint">
              Your agent asks with a checklist. You tick one from your phone.
            </p>
          </div>
          <img
            src="/humans-phone.webp"
            width={660}
            height={1386}
            loading="lazy"
            alt="APP-51 Pick the launch date on a phone. The agent's checklist offers Oct 20 (after the beta), Oct 27 (gives docs a week) and Nov 3, with Oct 27 ticked. Activity shows Blake changed the description just now, after opencode-blake created the issue and assigned it to him."
            className="relative z-10 order-2 mx-auto mt-6 block h-auto w-[220px] max-w-none drop-shadow-[0_24px_40px_rgb(0_0_0/0.55)] md:-ml-4 md:mt-[calc((100vw-3rem)*0.22)] md:w-[250px] lg:-ml-6 lg:mt-[213px] lg:w-[300px] xl:mt-[229px]"
          />
        </div>
      </Reveal>
    </section>
  );
}
