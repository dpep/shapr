class W
  def initialize
    @speed = 1
  end

  def s
    @speed
  end

  def g(n)
    instance_variable_get(n)
  end
end
