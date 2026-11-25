class Client
  def call(env)
    env.headers.each do |name, value|
      log(name, value)
    end
    puts "params"
    env.params.each do |name, value|
      record(name.to_s, value)
    end
    finish(env)
  end
end
