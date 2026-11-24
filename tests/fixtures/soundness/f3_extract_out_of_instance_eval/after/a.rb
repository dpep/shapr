class A
  def run(obj)
    obj.instance_eval do
      setup
    end
  end

  def setup
    @a = 1
    @b = 2
    @c = @a + @b
  end
end
