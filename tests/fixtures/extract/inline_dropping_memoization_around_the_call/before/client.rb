class Client
  def call(env)
    session = @session ||= build_session
    session.get(env)
  end

  def build_session
    session = Session.new
    @setup.call(session) if @setup
    session
  end
end
