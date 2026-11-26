const info=await api.state();
return {savedAccent:info.cfg.dashboard_accent};
