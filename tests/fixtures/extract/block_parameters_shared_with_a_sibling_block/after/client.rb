class Client
  def call(env)
    env.headers.each do |name, value|
      log(name, value)
    end
    record_params(env)
    finish(env)
  end

  def record_params(env)
    puts "params"
    env.params.each do |name, value|
      record(name.to_s, value)
    end
  end
end
