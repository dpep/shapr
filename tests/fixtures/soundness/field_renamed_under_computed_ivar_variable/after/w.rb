class W
  def initialize
    @pace = 1
  end

  def s
    @pace
  end

  def g(n)
    instance_variable_get(n)
  end
end
