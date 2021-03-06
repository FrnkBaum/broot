use {
    super::*,
    crate::{
        command::*,
        display::{status_line, Areas, Screen, W, WIDE_STATUS, write_flags},
        errors::ProgramError,
        keys,
        skin::PanelSkin,
        task_sync::Dam,
        verb::*,
    },
    minimad::{Alignment, Composite},
    std::path::PathBuf,
    termimad::Event,
};

pub struct Panel {
    id: PanelId,
    states: Vec<Box<dyn AppState>>, // stack: the last one is current
    pub areas: Areas,
    status: Option<Status>,
    pub purpose: PanelPurpose,
    input: PanelInput,
}

impl Panel {

    pub fn new(
        id: PanelId,
        state: Box<dyn AppState>,
        areas: Areas,
        _con: &AppContext,
    ) -> Self {
        let mut input = PanelInput::new(areas.input.clone());
        input.set_content(&state.get_starting_input());
        Self {
            id,
            states: vec![state],
            areas,
            status: None,
            purpose: PanelPurpose::None,
            input,
        }
    }

    pub fn set_error(&mut self, text: String) {
        self.status = Some(Status::from_error(text));
    }

    pub fn apply_command(
        &mut self,
        w: &mut W,
        cmd: &Command,
        other_path: &Option<PathBuf>,
        screen: &mut Screen,
        panel_skin: &PanelSkin,
        con: &AppContext,
    ) -> Result<AppStateCmdResult, ProgramError> {
        let state_idx = self.states.len()-1;
        let cc = CmdContext {
            cmd,
            other_path,
            panel_skin,
            con,
            areas: &self.areas,
            panel_purpose: self.purpose,
        };
        let result = self.states[state_idx].on_command(w, &cc, screen);
        self.status = Some(self.state().get_status(cmd, other_path, con));
        debug!("result in panel {:?}: {:?}", &self.id, &result);
        result
    }

    /// called on focusing the panel and before the display,
    /// this updates the status from the command read in the input
    pub fn refresh_input_status(
        &mut self,
        other_path: &Option<PathBuf>,
        con: &AppContext,
    ) {
        let cmd = Command::from_raw(self.input.get_content(), false);
        self.status = Some(self.state().get_status(&cmd, other_path, con));
    }

    /// execute all the pending tasks until there's none remaining or
    ///  the dam asks for interruption
    pub fn do_pending_tasks(
        &mut self,
        screen: &mut Screen,
        con: &AppContext,
        dam: &mut Dam,
    ) -> Result<(), ProgramError> {
        while self.mut_state().get_pending_task().is_some() & !dam.has_event() {
            self.mut_state().do_pending_task(screen, con, dam);
        }
        Ok(())
    }

    /// return a new command
    /// Update the input field
    pub fn add_event(
        &mut self,
        w: &mut W,
        event: Event,
        con: &AppContext,
    ) -> Result<Command, ProgramError> {
        self.input.on_event(w, event, con, &*self.states[self.states.len()-1])
    }

    pub fn push_state(&mut self, new_state: Box<dyn AppState>) {
        self.input.set_content(&new_state.get_starting_input());
        self.states.push(new_state);
    }
    pub fn mut_state(&mut self) -> &mut dyn AppState {
        self.states.last_mut().unwrap().as_mut()
    }
    pub fn state(&self) -> &dyn AppState {
        self.states.last().unwrap().as_ref()
    }

    pub fn clear_input(&mut self) {
        self.input.set_content("");
    }

    pub fn set_input_content(&mut self, content: &str) {
        self.input.set_content(content);
    }

    pub fn get_input_content(&self) -> String {
        self.input.get_content()
    }

    pub fn set_input_arg(&mut self, arg: String) {
        let mut command_parts = CommandParts::from(self.input.get_content());
        if let Some(invocation) = &mut command_parts.verb_invocation {
            invocation.args = Some(arg);
            let new_input = format!("{}", command_parts);
            self.input.set_content(&new_input);
        }
    }

    /// return true when the element has been removed
    pub fn remove_state(&mut self) -> bool {
        if self.states.len() > 1 {
            self.states.pop();
            self.input.set_content(&self.state().get_starting_input());
            true
        } else {
            false
        }
    }

    pub fn display(
        &mut self,
        w: &mut W,
        active: bool,
        screen: &mut Screen,
        panel_skin: &PanelSkin,
        con: &AppContext,
    ) -> Result<(), ProgramError> {
        let state_area = self.areas.state.clone();
        self.mut_state().display(w, screen, state_area, panel_skin, con)?;
        if active || !WIDE_STATUS {
            self.write_status(w, panel_skin, screen)?;
        }
        self.input.display(w, active, self.areas.input.clone(), panel_skin)?;
        if active {
            self.write_purpose(w, panel_skin, screen, con)?;
            let flags = self.state().get_flags();
            let input_content_len = self.input.get_content().len() as u16;
            write_flags(w, &flags, &self.areas.input, input_content_len, screen, panel_skin)?;
        }
        Ok(())
    }

    fn write_status(
        &self,
        w: &mut W,
        panel_skin: &PanelSkin,
        screen: &Screen,
    ) -> Result<(), ProgramError> {
        let task = self.state().get_pending_task();
        lazy_static! {
            static ref DEFAULT_STATUS: Status = Status::from_message(
                "Hit *esc* to go back, *enter* to go up, *?* for help, or a few letters to search"
            );
        }
        let status = self.status.as_ref().unwrap_or(&*DEFAULT_STATUS);
        status_line::write(w, task, status, &self.areas.status, panel_skin, screen)
    }

    fn write_purpose(
        &self,
        w: &mut W,
        panel_skin: &PanelSkin,
        screen: &Screen,
        con: &AppContext,
    ) -> Result<(), ProgramError> {
        if !self.purpose.is_arg_edition() {
            return Ok(());
        }
        if let Some(area) = &self.areas.purpose {
            let shortcut = con.verb_store.verbs
                .iter()
                .filter(|v| match &v.execution {
                    VerbExecution::Internal(exec) => exec.internal == Internal::start_end_panel,
                    _ => false,
                })
                .filter_map(|v| v.keys.first())
                .map(|&k| keys::key_event_desc(k))
                .next()
                .unwrap_or_else(|| ":start_end_panel".to_string());

            let md = format!("hit *{}* to fill arg ", shortcut);
            // Add verbindex in purpose ?
            screen.goto(w, area.left, area.top)?;
            panel_skin.purpose_skin.write_composite_fill(
                w,
                Composite::from_inline(&md),
                area.width as usize,
                Alignment::Right,
            )?;
        }
        Ok(())
    }

}
