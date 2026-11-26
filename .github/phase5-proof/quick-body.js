const tool=await until(()=>shown('grammar')?'grammar':shown('translation')?'translation':null,'actual quick route');
const initial=input(tool).value;
if(initial)assert(initial==='Synthetic phase five selection','UI Automation returned the wrong selected text');
return {tool,initial,flow:await textFlow(tool,{manual:!initial,quick:true})};
