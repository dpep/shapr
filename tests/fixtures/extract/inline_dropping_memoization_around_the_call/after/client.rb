class Client
  def call(env)
    session = Session.new
    @setup.call(session) if @setup
    session.get(env)
  end
end
