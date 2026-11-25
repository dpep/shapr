# typed: true
class A
  def run
    ~T.unsafe(self)
  end

  private

  def -@; 1; end
  def ~; 2; end
  def !; false; end
end
