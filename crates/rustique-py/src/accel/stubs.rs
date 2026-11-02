//! First-call stubs (validate on first call, docs/ACCELERATION.md §3.2): while a target is
//! *armed*, each of its pointer-cell sites (`ml_meth`, slots) holds one of these stubs, and
//! its vectorcall sites hold [`super::registry::stub_vectorcall`]. A stub forwards its
//! arguments unchanged: it asks the registry to validate the target (prepare in full, pins,
//! the kernel swapped in) and then calls whatever the site holds now (the kernel, or the
//! original when the target refused or could not be validated right now).
//!
//! One stub per armed site, by C arity: arguments and results of every entry point the
//! framework patches are pointer-sized integers or pointers, or `int`s passed and returned
//! in the same registers, so a stub of arity `n` is declared over `usize` and forwards the
//! raw registers (x86-64 SysV/Win64, AArch64). `POOL` stubs per arity; the registry hands
//! them out and takes them back.

pub const POOL: usize = 128;
pub const ARITIES: usize = 5;

unsafe extern "C" fn s1<const K: usize>(a0: usize) -> usize {
    let f = super::registry::cell_dispatch(K);
    // SAFETY: `f` is the site's current entry point (or its original), of this arity.
    unsafe { std::mem::transmute::<usize, unsafe extern "C" fn(usize) -> usize>(f)(a0) }
}

unsafe extern "C" fn s2<const K: usize>(a0: usize, a1: usize) -> usize {
    let f = super::registry::cell_dispatch(K);
    // SAFETY: `f` is the site's current entry point (or its original), of this arity.
    unsafe { std::mem::transmute::<usize, unsafe extern "C" fn(usize, usize) -> usize>(f)(a0, a1) }
}

unsafe extern "C" fn s3<const K: usize>(a0: usize, a1: usize, a2: usize) -> usize {
    let f = super::registry::cell_dispatch(K);
    // SAFETY: `f` is the site's current entry point (or its original), of this arity.
    unsafe { std::mem::transmute::<usize, unsafe extern "C" fn(usize, usize, usize) -> usize>(f)(a0, a1, a2) }
}

unsafe extern "C" fn s4<const K: usize>(a0: usize, a1: usize, a2: usize, a3: usize) -> usize {
    let f = super::registry::cell_dispatch(K);
    // SAFETY: `f` is the site's current entry point (or its original), of this arity.
    unsafe {
        std::mem::transmute::<usize, unsafe extern "C" fn(usize, usize, usize, usize) -> usize>(f)(a0, a1, a2, a3)
    }
}

unsafe extern "C" fn s5<const K: usize>(a0: usize, a1: usize, a2: usize, a3: usize, a4: usize) -> usize {
    let f = super::registry::cell_dispatch(K);
    // SAFETY: `f` is the site's current entry point (or its original), of this arity.
    unsafe {
        std::mem::transmute::<usize, unsafe extern "C" fn(usize, usize, usize, usize, usize) -> usize>(f)(
            a0, a1, a2, a3, a4,
        )
    }
}

static S1: [unsafe extern "C" fn(usize) -> usize; POOL] = [
    s1::<0>, s1::<1>, s1::<2>, s1::<3>, s1::<4>, s1::<5>, s1::<6>, s1::<7>, s1::<8>, s1::<9>, s1::<10>, s1::<11>,
    s1::<12>, s1::<13>, s1::<14>, s1::<15>, s1::<16>, s1::<17>, s1::<18>, s1::<19>, s1::<20>, s1::<21>, s1::<22>,
    s1::<23>, s1::<24>, s1::<25>, s1::<26>, s1::<27>, s1::<28>, s1::<29>, s1::<30>, s1::<31>, s1::<32>, s1::<33>,
    s1::<34>, s1::<35>, s1::<36>, s1::<37>, s1::<38>, s1::<39>, s1::<40>, s1::<41>, s1::<42>, s1::<43>, s1::<44>,
    s1::<45>, s1::<46>, s1::<47>, s1::<48>, s1::<49>, s1::<50>, s1::<51>, s1::<52>, s1::<53>, s1::<54>, s1::<55>,
    s1::<56>, s1::<57>, s1::<58>, s1::<59>, s1::<60>, s1::<61>, s1::<62>, s1::<63>, s1::<64>, s1::<65>, s1::<66>,
    s1::<67>, s1::<68>, s1::<69>, s1::<70>, s1::<71>, s1::<72>, s1::<73>, s1::<74>, s1::<75>, s1::<76>, s1::<77>,
    s1::<78>, s1::<79>, s1::<80>, s1::<81>, s1::<82>, s1::<83>, s1::<84>, s1::<85>, s1::<86>, s1::<87>, s1::<88>,
    s1::<89>, s1::<90>, s1::<91>, s1::<92>, s1::<93>, s1::<94>, s1::<95>, s1::<96>, s1::<97>, s1::<98>, s1::<99>,
    s1::<100>, s1::<101>, s1::<102>, s1::<103>, s1::<104>, s1::<105>, s1::<106>, s1::<107>, s1::<108>, s1::<109>,
    s1::<110>, s1::<111>, s1::<112>, s1::<113>, s1::<114>, s1::<115>, s1::<116>, s1::<117>, s1::<118>, s1::<119>,
    s1::<120>, s1::<121>, s1::<122>, s1::<123>, s1::<124>, s1::<125>, s1::<126>, s1::<127>,
];
static S2: [unsafe extern "C" fn(usize, usize) -> usize; POOL] = [
    s2::<128>, s2::<129>, s2::<130>, s2::<131>, s2::<132>, s2::<133>, s2::<134>, s2::<135>, s2::<136>, s2::<137>,
    s2::<138>, s2::<139>, s2::<140>, s2::<141>, s2::<142>, s2::<143>, s2::<144>, s2::<145>, s2::<146>, s2::<147>,
    s2::<148>, s2::<149>, s2::<150>, s2::<151>, s2::<152>, s2::<153>, s2::<154>, s2::<155>, s2::<156>, s2::<157>,
    s2::<158>, s2::<159>, s2::<160>, s2::<161>, s2::<162>, s2::<163>, s2::<164>, s2::<165>, s2::<166>, s2::<167>,
    s2::<168>, s2::<169>, s2::<170>, s2::<171>, s2::<172>, s2::<173>, s2::<174>, s2::<175>, s2::<176>, s2::<177>,
    s2::<178>, s2::<179>, s2::<180>, s2::<181>, s2::<182>, s2::<183>, s2::<184>, s2::<185>, s2::<186>, s2::<187>,
    s2::<188>, s2::<189>, s2::<190>, s2::<191>, s2::<192>, s2::<193>, s2::<194>, s2::<195>, s2::<196>, s2::<197>,
    s2::<198>, s2::<199>, s2::<200>, s2::<201>, s2::<202>, s2::<203>, s2::<204>, s2::<205>, s2::<206>, s2::<207>,
    s2::<208>, s2::<209>, s2::<210>, s2::<211>, s2::<212>, s2::<213>, s2::<214>, s2::<215>, s2::<216>, s2::<217>,
    s2::<218>, s2::<219>, s2::<220>, s2::<221>, s2::<222>, s2::<223>, s2::<224>, s2::<225>, s2::<226>, s2::<227>,
    s2::<228>, s2::<229>, s2::<230>, s2::<231>, s2::<232>, s2::<233>, s2::<234>, s2::<235>, s2::<236>, s2::<237>,
    s2::<238>, s2::<239>, s2::<240>, s2::<241>, s2::<242>, s2::<243>, s2::<244>, s2::<245>, s2::<246>, s2::<247>,
    s2::<248>, s2::<249>, s2::<250>, s2::<251>, s2::<252>, s2::<253>, s2::<254>, s2::<255>,
];
static S3: [unsafe extern "C" fn(usize, usize, usize) -> usize; POOL] = [
    s3::<256>, s3::<257>, s3::<258>, s3::<259>, s3::<260>, s3::<261>, s3::<262>, s3::<263>, s3::<264>, s3::<265>,
    s3::<266>, s3::<267>, s3::<268>, s3::<269>, s3::<270>, s3::<271>, s3::<272>, s3::<273>, s3::<274>, s3::<275>,
    s3::<276>, s3::<277>, s3::<278>, s3::<279>, s3::<280>, s3::<281>, s3::<282>, s3::<283>, s3::<284>, s3::<285>,
    s3::<286>, s3::<287>, s3::<288>, s3::<289>, s3::<290>, s3::<291>, s3::<292>, s3::<293>, s3::<294>, s3::<295>,
    s3::<296>, s3::<297>, s3::<298>, s3::<299>, s3::<300>, s3::<301>, s3::<302>, s3::<303>, s3::<304>, s3::<305>,
    s3::<306>, s3::<307>, s3::<308>, s3::<309>, s3::<310>, s3::<311>, s3::<312>, s3::<313>, s3::<314>, s3::<315>,
    s3::<316>, s3::<317>, s3::<318>, s3::<319>, s3::<320>, s3::<321>, s3::<322>, s3::<323>, s3::<324>, s3::<325>,
    s3::<326>, s3::<327>, s3::<328>, s3::<329>, s3::<330>, s3::<331>, s3::<332>, s3::<333>, s3::<334>, s3::<335>,
    s3::<336>, s3::<337>, s3::<338>, s3::<339>, s3::<340>, s3::<341>, s3::<342>, s3::<343>, s3::<344>, s3::<345>,
    s3::<346>, s3::<347>, s3::<348>, s3::<349>, s3::<350>, s3::<351>, s3::<352>, s3::<353>, s3::<354>, s3::<355>,
    s3::<356>, s3::<357>, s3::<358>, s3::<359>, s3::<360>, s3::<361>, s3::<362>, s3::<363>, s3::<364>, s3::<365>,
    s3::<366>, s3::<367>, s3::<368>, s3::<369>, s3::<370>, s3::<371>, s3::<372>, s3::<373>, s3::<374>, s3::<375>,
    s3::<376>, s3::<377>, s3::<378>, s3::<379>, s3::<380>, s3::<381>, s3::<382>, s3::<383>,
];
static S4: [unsafe extern "C" fn(usize, usize, usize, usize) -> usize; POOL] = [
    s4::<384>, s4::<385>, s4::<386>, s4::<387>, s4::<388>, s4::<389>, s4::<390>, s4::<391>, s4::<392>, s4::<393>,
    s4::<394>, s4::<395>, s4::<396>, s4::<397>, s4::<398>, s4::<399>, s4::<400>, s4::<401>, s4::<402>, s4::<403>,
    s4::<404>, s4::<405>, s4::<406>, s4::<407>, s4::<408>, s4::<409>, s4::<410>, s4::<411>, s4::<412>, s4::<413>,
    s4::<414>, s4::<415>, s4::<416>, s4::<417>, s4::<418>, s4::<419>, s4::<420>, s4::<421>, s4::<422>, s4::<423>,
    s4::<424>, s4::<425>, s4::<426>, s4::<427>, s4::<428>, s4::<429>, s4::<430>, s4::<431>, s4::<432>, s4::<433>,
    s4::<434>, s4::<435>, s4::<436>, s4::<437>, s4::<438>, s4::<439>, s4::<440>, s4::<441>, s4::<442>, s4::<443>,
    s4::<444>, s4::<445>, s4::<446>, s4::<447>, s4::<448>, s4::<449>, s4::<450>, s4::<451>, s4::<452>, s4::<453>,
    s4::<454>, s4::<455>, s4::<456>, s4::<457>, s4::<458>, s4::<459>, s4::<460>, s4::<461>, s4::<462>, s4::<463>,
    s4::<464>, s4::<465>, s4::<466>, s4::<467>, s4::<468>, s4::<469>, s4::<470>, s4::<471>, s4::<472>, s4::<473>,
    s4::<474>, s4::<475>, s4::<476>, s4::<477>, s4::<478>, s4::<479>, s4::<480>, s4::<481>, s4::<482>, s4::<483>,
    s4::<484>, s4::<485>, s4::<486>, s4::<487>, s4::<488>, s4::<489>, s4::<490>, s4::<491>, s4::<492>, s4::<493>,
    s4::<494>, s4::<495>, s4::<496>, s4::<497>, s4::<498>, s4::<499>, s4::<500>, s4::<501>, s4::<502>, s4::<503>,
    s4::<504>, s4::<505>, s4::<506>, s4::<507>, s4::<508>, s4::<509>, s4::<510>, s4::<511>,
];
static S5: [unsafe extern "C" fn(usize, usize, usize, usize, usize) -> usize; POOL] = [
    s5::<512>, s5::<513>, s5::<514>, s5::<515>, s5::<516>, s5::<517>, s5::<518>, s5::<519>, s5::<520>, s5::<521>,
    s5::<522>, s5::<523>, s5::<524>, s5::<525>, s5::<526>, s5::<527>, s5::<528>, s5::<529>, s5::<530>, s5::<531>,
    s5::<532>, s5::<533>, s5::<534>, s5::<535>, s5::<536>, s5::<537>, s5::<538>, s5::<539>, s5::<540>, s5::<541>,
    s5::<542>, s5::<543>, s5::<544>, s5::<545>, s5::<546>, s5::<547>, s5::<548>, s5::<549>, s5::<550>, s5::<551>,
    s5::<552>, s5::<553>, s5::<554>, s5::<555>, s5::<556>, s5::<557>, s5::<558>, s5::<559>, s5::<560>, s5::<561>,
    s5::<562>, s5::<563>, s5::<564>, s5::<565>, s5::<566>, s5::<567>, s5::<568>, s5::<569>, s5::<570>, s5::<571>,
    s5::<572>, s5::<573>, s5::<574>, s5::<575>, s5::<576>, s5::<577>, s5::<578>, s5::<579>, s5::<580>, s5::<581>,
    s5::<582>, s5::<583>, s5::<584>, s5::<585>, s5::<586>, s5::<587>, s5::<588>, s5::<589>, s5::<590>, s5::<591>,
    s5::<592>, s5::<593>, s5::<594>, s5::<595>, s5::<596>, s5::<597>, s5::<598>, s5::<599>, s5::<600>, s5::<601>,
    s5::<602>, s5::<603>, s5::<604>, s5::<605>, s5::<606>, s5::<607>, s5::<608>, s5::<609>, s5::<610>, s5::<611>,
    s5::<612>, s5::<613>, s5::<614>, s5::<615>, s5::<616>, s5::<617>, s5::<618>, s5::<619>, s5::<620>, s5::<621>,
    s5::<622>, s5::<623>, s5::<624>, s5::<625>, s5::<626>, s5::<627>, s5::<628>, s5::<629>, s5::<630>, s5::<631>,
    s5::<632>, s5::<633>, s5::<634>, s5::<635>, s5::<636>, s5::<637>, s5::<638>, s5::<639>,
];

/// The address of stub `k` (global index: arity `(k / POOL) + 1`).
pub fn addr(k: usize) -> usize {
    let (a, i) = (k / POOL + 1, k % POOL);
    match a {
        1 => S1[i] as usize,
        2 => S2[i] as usize,
        3 => S3[i] as usize,
        4 => S4[i] as usize,
        _ => S5[i] as usize,
    }
}
